//! 截屏与贴图在主界面这一侧的流程：响应快捷键和菜单，截屏后显示截屏遮罩（`snipper`），
//! 处理截屏结果（复制、保存、贴图、提取文字），维持贴图窗口（`pin`）并执行它们交上来的操作。
//!
//! 截屏遮罩和贴图都是 egui 的子视口，只能在主界面的 `ui` 里创建；面板隐藏时 eframe 只调用 `logic`，
//! 所以创建前先用 `panel::wake` 让下一帧运行界面（见那里的说明）。

use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use egui::{Pos2, Rect, Vec2};
use image::RgbaImage;

use super::native_menu::NativeMenu;
use super::pin::{Pin, PinAction};
use super::snipper::{self, Outcome, Snipper};
use super::HubApp;
use crate::controller;
use crate::platform::{self, floating, panel as native_panel, screen};
use crate::shared::{AppEvent, PinRequest};
use crate::snip::{self, Capture};

/// 编辑贴图的请求：截屏完成后用它打开编辑（贴图的图片和窗口位置）。
pub(super) struct PinEdit {
    pin: u64,
    image: Arc<RgbaImage>,
    /// 贴图窗口在桌面上的位置（逻辑坐标）。
    rect: Rect,
}

/// 截屏结束后遮罩至少停留多久。
const CLOSE_DELAY: Duration = Duration::from_millis(200);
/// 原来的应用回到前台后，遮罩再停留多久：等它把窗口重画成活跃的样子。
const CLOSE_SETTLE: Duration = Duration::from_millis(60);
/// 遮罩最多停留多久（原来的应用一直没回到前台时）。
const CLOSE_TIMEOUT: Duration = Duration::from_millis(1000);
/// 等原来的应用回到前台时，多久检查一次。
const CLOSE_POLL: Duration = Duration::from_millis(16);

/// 截屏刚结束、正在收起的遮罩：先把焦点还给原来的应用，等它恢复成活跃的样子再收起，
/// 期间遮罩只画静止的截图（见 `floating::restore_focus`）。
pub(super) struct ClosingSnip {
    snipper: Snipper,
    started: Instant,
    /// 第一次看到原来的应用回到前台的时间。
    restored: Option<Instant>,
}

/// 后台截屏的结果：截图与截屏时鼠标的位置（全局坐标）。
pub(super) type CaptureResult = anyhow::Result<(Capture, [f64; 2])>;

/// 剪贴板里的图片贴出来时，最大占屏幕的比例（太大的图片先缩小显示，菜单里可以恢复 100%）。
const MAX_PIN_FRACTION: f32 = 0.8;

impl HubApp {
    /// 截屏：截下鼠标所在的显示器，记下屏幕上的窗口，然后显示截屏遮罩。
    pub(super) fn start_snip(&mut self) {
        self.begin_capture(None);
    }

    /// 开始截屏；`edit` 不为空时截屏后编辑这张贴图。截屏或编辑进行中时忽略。
    fn begin_capture(&mut self, edit: Option<PinEdit>) {
        if self.snip.is_some() || self.capturing.is_some() || self.closing_snip.is_some() {
            return;
        }
        if !screen::capture_allowed() && !screen::request_capture_access() {
            tracing::warn!("没有屏幕录制权限，无法截屏");
            self.show_capture_help("截屏需要“屏幕录制”权限");
            return;
        }
        let Some((cursor, display)) = screen::cursor() else {
            return;
        };
        if let Some(edit) = &edit {
            // 截的是鼠标所在的显示器，贴图要在这个显示器上（用菜单或空格编辑时鼠标一般就在贴图上）。
            if !display.logical.contains(edit.rect.center()) {
                tracing::debug!("贴图不在鼠标所在的显示器上，不编辑");
                screen::beep();
                return;
            }
        }
        self.pending_edit = edit;
        // 截屏放在后台线程：系统可能要先确认权限（macOS 15 起旧的截屏接口经 ScreenCaptureKit 代理，
        // 实测新编译的程序第一次截屏会等好几秒、最后返回空），这期间界面、菜单和快捷键照常响应。
        // 先取窗口位置再截图，两者都在截屏遮罩出现之前完成，遮罩不会出现在截图里。
        let (sender, receiver) = mpsc::channel();
        self.capturing = Some(receiver);
        let shared = self.shared.clone();
        std::thread::Builder::new()
            .name("screen-capture".into())
            .spawn(move || {
                let started = Instant::now();
                let windows = screen::windows();
                let result = screen::capture(&display).map(|image| {
                    let capture = Capture::new(display, image, &windows);
                    tracing::info!(
                        "截屏：显示器 {:?}（缩放 {}），图像 {}×{}，识别到 {} 个窗口，用时 {:?}",
                        capture.display.frame,
                        capture.display.scale,
                        capture.image.width(),
                        capture.image.height(),
                        capture.windows.len(),
                        started.elapsed()
                    );
                    (capture, cursor)
                });
                let _ = sender.send(result);
                shared.post(AppEvent::SnipCaptured);
            })
            .expect("创建截屏线程失败");
    }

    /// 后台截屏完成：显示截屏遮罩；失败时打开设置页说明怎么授权。
    pub(super) fn finish_capture(&mut self) {
        let Some(result) = self.capturing.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        self.capturing = None;
        let edit = self.pending_edit.take();
        match result {
            Ok((capture, cursor)) => {
                self.snip = Some(match edit {
                    Some(edit) => {
                        Snipper::edit_pin(capture, cursor, edit.pin, edit.image, edit.rect)
                    }
                    None => Snipper::new(capture, cursor),
                });
                self.request_ui_pass();
            }
            Err(err) => {
                tracing::warn!("截屏失败：{err:#}");
                self.show_capture_help("截屏失败：请确认已允许 DevTools Hub 录屏");
            }
        }
    }

    /// 打开设置页的“截屏与贴图”分组（屏幕录制权限在那里），并提示原因。
    fn show_capture_help(&mut self, message: &str) {
        if !platform::SCREEN_CAPTURE_NEEDS_PERMISSION {
            screen::beep();
            return;
        }
        controller::show_panel(&self.shared, true);
        self.settings_view.reveal_snip = true;
        self.show_toast(message);
    }

    /// 让下一帧运行界面（面板隐藏时，见 `panel::wake`）。
    fn request_ui_pass(&self) {
        if !native_panel::is_visible() {
            native_panel::wake();
        }
        self.ctx.request_repaint();
    }

    /// 在 `ui` 里显示截屏遮罩，截屏结束后处理结果。
    pub(super) fn snip_ui(&mut self, ctx: &egui::Context) {
        let Some(snipper) = &self.snip else {
            return;
        };
        let outcome = snipper.show(ctx);
        if matches!(outcome, Outcome::Continue) {
            return;
        }
        // 编辑贴图时：画了标注就把结果写回贴图（复制、保存、完成都算），没画就不动它。
        let editing = snipper.pin();
        let edited = editing.filter(|_| snipper.has_marks());
        let Some(snipper) = self.snip.take() else {
            return;
        };
        if matches!(outcome, Outcome::Save(_) | Outcome::Ocr(_)) {
            // 保存对话框要显示在遮罩上面：立即收起遮罩，焦点由保存对话框结束时还回去。
            // 提取文字的结果显示在主窗口里，同样立即收起遮罩，焦点在主窗口收起时还回去。
            floating::hide_overlay(snipper.title());
        } else {
            // 先把焦点还给原来的应用，遮罩（这时只画静止的截图）再停留一会儿，见 `floating::restore_focus`。
            floating::restore_focus();
            self.closing_snip = Some(ClosingSnip {
                snipper,
                started: Instant::now(),
                restored: None,
            });
            ctx.request_repaint();
        }
        match outcome {
            Outcome::Continue | Outcome::Cancel => {}
            Outcome::Copy(image) => {
                self.update_pin(edited, &image);
                self.copy_image(&image);
            }
            Outcome::Save(image) => {
                self.update_pin(edited, &image);
                self.save_image(&image);
                // macOS 的保存对话框已经还过焦点（这里什么也不做）；Windows 在这里还。
                floating::restore_focus();
            }
            Outcome::Pin { image, .. } if editing.is_some() => self.update_pin(edited, &image),
            Outcome::Pin { image, rect } => {
                let (zoom, top_left) = fit_long_pin(rect);
                self.create_pin(image, rect.size(), zoom, top_left);
            }
            Outcome::Ocr(image) => self.open_ocr(Arc::new(image)),
            Outcome::CopyColor(hex) => {
                let snapshot = devtools_clipboard::Snapshot::from_text(&hex);
                self.pasteboard().write(&snapshot);
            }
        }
    }

    /// 看门狗收起了卡住的遮罩（见 `snipper::watchdog`）：丢掉截屏状态，下次截屏照常开始。
    /// eframe 随后在运行界面时销毁没人维持的遮罩窗口。
    pub(super) fn check_snip_rescued(&mut self) {
        if snipper::take_rescued() {
            self.snip = None;
            self.closing_snip = None;
            self.pending_edit = None;
        }
    }

    /// 截屏结束后的过渡：遮罩只画静止的截图，等原来的应用恢复成活跃状态后再收起（egui 随后销毁窗口）。
    pub(super) fn closing_snip_ui(&mut self, ctx: &egui::Context) {
        let Some(closing) = &mut self.closing_snip else {
            return;
        };
        let now = Instant::now();
        if closing.restored.is_none() && floating::focus_restored() {
            closing.restored = Some(now);
        }
        let timeout = closing.started + CLOSE_TIMEOUT;
        let until = closing
            .restored
            .map_or(timeout, |at| {
                (at + CLOSE_SETTLE).max(closing.started + CLOSE_DELAY)
            })
            .min(timeout);
        if now < until {
            let _ = closing.snipper.show(ctx);
            let wait = until - now;
            // 还没回到前台时每帧检查一次。
            let wait = if closing.restored.is_some() {
                wait
            } else {
                wait.min(CLOSE_POLL)
            };
            ctx.request_repaint_after(wait);
        } else {
            match closing.restored {
                Some(at) => tracing::debug!(
                    "截屏结束后 {:?} 原来的应用回到前台，{:?} 后收起遮罩",
                    at - closing.started,
                    now - closing.started
                ),
                None => tracing::warn!("截屏结束后原来的应用一直没回到前台，超时收起遮罩"),
            }
            floating::hide_overlay(closing.snipper.title());
            self.closing_snip = None;
        }
    }

    /// 把编辑后的图片写回贴图（按贴图现在在屏幕上的大小显示）。
    fn update_pin(&mut self, pin: Option<u64>, image: &RgbaImage) {
        let Some(pin) = pin.and_then(|id| self.pins.iter().find(|pin| pin.id == id)) else {
            return;
        };
        if let Some(rect) = pin.screen_rect() {
            pin.replace_image(&self.ctx, image.clone(), rect.size());
        }
    }

    /// 把剪贴板里的图片贴到鼠标所在的位置（对应 Snipaste 的“贴图”，默认 F3）。没有图片时只响一声。
    pub(super) fn pin_clipboard(&mut self) {
        let snapshot = self.pasteboard().read();
        match snip::image_from_snapshot(&snapshot) {
            Some(image) => self.pin_at_cursor(image),
            None => {
                tracing::debug!("剪贴板里没有可以贴的图片");
                screen::beep();
            }
        }
    }

    /// 把图片贴在鼠标所在的显示器上，以鼠标为中心；比屏幕大时先缩小显示。
    pub(super) fn pin_at_cursor(&mut self, image: RgbaImage) {
        let Some((cursor, display)) = screen::cursor() else {
            return;
        };
        // 1 个图像像素对应 1 个物理像素，即 100%。
        let base = Vec2::new(image.width() as f32, image.height() as f32) / display.scale;
        let area = display.logical;
        let fit = (area.size() * MAX_PIN_FRACTION / base).min_elem().min(1.0);
        let size = base * fit;
        let center = display.to_logical(cursor);
        let min = Pos2::new(
            (center.x - size.x / 2.0).clamp(area.left(), (area.right() - size.x).max(area.left())),
            (center.y - size.y / 2.0).clamp(area.top(), (area.bottom() - size.y).max(area.top())),
        );
        self.create_pin(image, base, fit, min);
    }

    /// 创建贴图窗口。
    fn create_pin(&mut self, image: RgbaImage, base: Vec2, zoom: f32, top_left: Pos2) {
        let id = self.next_pin_id;
        self.next_pin_id += 1;
        tracing::info!(
            "贴图 #{id}：{}×{} 像素，位置 {top_left:?}，大小 {base:?} × {zoom:.2}",
            image.width(),
            image.height()
        );
        let max_side = self.ctx.input(|i| i.max_texture_side);
        self.pins.push(Pin::new_in_background(
            id, image, base, zoom, top_left, max_side,
        ));
        self.request_ui_pass();
    }

    /// 在 `ui` 里维持所有贴图窗口：关闭了的不再维持，egui 随后销毁它们的窗口。
    pub(super) fn pins_ui(&mut self, ctx: &egui::Context) {
        self.pins.retain(|pin| {
            let closed = pin.is_closed();
            if closed {
                floating::release_pin(&pin.title());
            }
            !closed
        });
        if let Some((id, _)) = &self.pin_menu {
            if !self.pins.iter().any(|pin| pin.id == *id) {
                self.pin_menu = None;
            }
        }
        for pin in &self.pins {
            pin.show(ctx, &self.shared);
        }
    }

    /// 关闭所有贴图（菜单栏菜单、贴图的右键菜单）。
    pub(super) fn close_pins(&mut self) {
        for pin in &self.pins {
            pin.apply(&self.ctx, PinAction::Close);
        }
    }

    /// 执行贴图窗口交上来的操作。
    pub(super) fn pin_request(&mut self, id: u64, request: PinRequest) {
        match request {
            PinRequest::Menu => self.pin_menu(id),
            PinRequest::Copy => self.pin_action(id, PinAction::Copy),
            PinRequest::Save => self.pin_action(id, PinAction::Save),
            PinRequest::Edit => self.pin_action(id, PinAction::Edit),
            PinRequest::Ocr => self.pin_action(id, PinAction::Ocr),
        }
    }

    /// 在贴图上弹出右键菜单。macOS 上选中的菜单项随后经 `MenuCommand` 送来，Windows 上直接返回。
    fn pin_menu(&mut self, id: u64) {
        let Some(pin) = self.pins.iter().find(|pin| pin.id == id) else {
            return;
        };
        let entries = pin.menu_entries();
        let title = pin.title();
        if let Some((menu, chosen)) = NativeMenu::popup_in(entries, &self.ctx, &title) {
            self.pin_menu = Some((id, menu));
            if let Some(chosen) = chosen {
                self.pin_menu_command(&chosen);
            }
        }
    }

    /// 贴图右键菜单的菜单项被选中；不是贴图菜单的菜单项时返回 false。
    pub(super) fn pin_menu_command(&mut self, item: &str) -> bool {
        let Some((id, menu)) = &mut self.pin_menu else {
            return false;
        };
        let id = *id;
        match menu.take(item) {
            Some(action) => {
                self.pin_action(id, action);
                true
            }
            None => false,
        }
    }

    /// 执行贴图的操作：复制、保存、关闭全部在这里处理，其余交给贴图自己。
    fn pin_action(&mut self, id: u64, action: PinAction) {
        let Some(index) = self.pins.iter().position(|pin| pin.id == id) else {
            return;
        };
        match action {
            PinAction::Copy => {
                let image = self.pins[index].image();
                self.copy_image(&image);
                self.pins[index].tip(&self.ctx, "已复制");
            }
            PinAction::Save => {
                let image = self.pins[index].image();
                if self.save_image(&image) {
                    // 保存对话框期间贴图可能已被关闭，重新查找。
                    if let Some(pin) = self.pins.iter().find(|pin| pin.id == id) {
                        pin.tip(&self.ctx, "已保存");
                    }
                }
            }
            PinAction::CloseAll => self.close_pins(),
            PinAction::Ocr => {
                let image = self.pins[index].image();
                self.open_ocr(image);
            }
            PinAction::Edit => {
                let pin = &self.pins[index];
                if let Some(rect) = pin.screen_rect() {
                    let edit = PinEdit {
                        pin: id,
                        image: pin.image(),
                        rect,
                    };
                    self.begin_capture(Some(edit));
                }
            }
            action => self.pins[index].apply(&self.ctx, action),
        }
    }

    /// 把图片复制到剪贴板。直接写系统剪贴板（不经 `Shared::write_pasteboard`），截图会照常记进剪贴板历史。
    fn copy_image(&mut self, image: &RgbaImage) {
        match snip::encode_png(image, true) {
            Ok(png) => {
                self.pasteboard().write(&snip::image_snapshot(png));
            }
            Err(err) => {
                tracing::warn!("复制截图失败：{err:#}");
                screen::beep();
            }
        }
    }

    /// 弹出保存对话框，把图片存为 PNG。返回是否保存成功（取消时为 false）。
    fn save_image(&mut self, image: &RgbaImage) -> bool {
        let directory = self
            .settings
            .screenshot_dir
            .clone()
            .filter(|dir| dir.is_dir())
            .or_else(default_screenshot_dir);
        let name = snip::file_name(chrono::Local::now());
        let Some(path) = floating::save_dialog(&name, directory.as_deref()) else {
            return false;
        };
        let result = snip::encode_png(image, false)
            .and_then(|png| std::fs::write(&path, png).context("写入文件失败"));
        match result {
            Ok(()) => {
                tracing::info!("截图已保存到 {}", path.display());
                let dir = path.parent().map(PathBuf::from);
                if dir != self.settings.screenshot_dir {
                    self.settings.screenshot_dir = dir;
                    self.persist_settings();
                }
                true
            }
            Err(err) => {
                tracing::warn!("保存截图失败（{}）：{err:#}", path.display());
                screen::beep();
                false
            }
        }
    }
}

/// 截屏结果贴到屏幕时的缩放比例与位置：普通截屏原样贴在截屏的地方；长截图比屏幕高，
/// 缩小到屏幕放得下（与贴剪贴板里的大图一样最多占 [`MAX_PIN_FRACTION`]），并保持在屏幕内。
fn fit_long_pin(rect: Rect) -> (f32, Pos2) {
    let Some((_, display)) = screen::cursor() else {
        return (1.0, rect.min);
    };
    let area = display.logical;
    if area.contains_rect(rect) {
        return (1.0, rect.min);
    }
    let zoom = (area.size() * MAX_PIN_FRACTION / rect.size())
        .min_elem()
        .min(1.0);
    let size = rect.size() * zoom;
    let top_left = Pos2::new(
        rect.min
            .x
            .clamp(area.left(), (area.right() - size.x).max(area.left())),
        rect.min
            .y
            .clamp(area.top(), (area.bottom() - size.y).max(area.top())),
    );
    (zoom, top_left)
}

/// 第一次保存截图时对话框打开的目录：macOS 是桌面（与系统截屏一致），Windows 是“图片”文件夹。
fn default_screenshot_dir() -> Option<PathBuf> {
    let dirs = directories::UserDirs::new()?;
    let dir = if cfg!(target_os = "macos") {
        dirs.desktop_dir()
    } else {
        dirs.picture_dir()
    };
    dir.map(PathBuf::from)
}
