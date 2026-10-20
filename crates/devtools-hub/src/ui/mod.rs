//! egui 界面：剪贴板面板、设置页、鼠标中键快捷面板与 JSON 编辑器，以及截屏遮罩和贴图窗口。
//!
//! 四个页面共用同一个窗口：切换页面时由平台层调整窗口大小和位置。
//! 截屏遮罩（`snipper`）和贴图（`pin`）是各自独立的窗口（egui 的子视口），流程在 `screenshot` 里。

mod code_text;
mod fonts;
mod glass;
mod icons;
mod json_editor;
mod liquid_glass;
mod menu;
mod native_menu;
mod panel;
mod pin;
mod quick_panel;
mod screenshot;
mod settings_page;
#[cfg(test)]
mod snapshot_tests;
mod snipper;
mod style;
mod textures;
mod widgets;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use devtools_clipboard::{uti, Snapshot, Store};

use crate::hotkey::{HotkeyAction, HotkeyService};
use crate::platform::pasteboard::Pasteboard;
use crate::platform::{keyboard, panel as native_panel, Anchor, Backdrop, Placement};
use crate::selection::{self, Selection};
use crate::settings::Settings;
use crate::shared::{AppEvent, Shared};
use crate::tray::Tray;
use crate::update::{self, Release};
use crate::{controller, middle_click, Args};

use code_text::CodeStyles;
use json_editor::JsonEditorView;
use liquid_glass::LiquidGlass;
use native_menu::NativeMenu;
use panel::PanelView;
use pin::{Pin, PinAction};
use quick_panel::QuickPanelView;
use settings_page::SettingsView;
use snipper::Snipper;
use style::Palette;
use textures::{AppIcons, Textures};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 窗口当前显示的页面。
enum Page {
    /// 剪贴板历史面板。
    Panel,
    Settings,
    /// 鼠标中键弹出的快捷面板。
    QuickActions,
    JsonEditor,
}

/// 粘贴方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteMode {
    /// 写回全部原始类型（保留格式）。
    Original,
    /// 只写入纯文本（对应 PasteNow 的 “Paste as Plain Text”）。
    PlainText,
    /// 只复制到剪贴板，不模拟 ⌘V。
    CopyOnly,
}

/// 窗口底部短暂显示的提示。
struct Toast {
    text: String,
    /// 到这个时间自动消失。
    until: Instant,
}

/// 数字键 1–9：面板上 ⌘1–9 快速粘贴、快捷面板上直接执行第 N 个功能。
const NUMBER_KEYS: [egui::Key; 9] = [
    egui::Key::Num1,
    egui::Key::Num2,
    egui::Key::Num3,
    egui::Key::Num4,
    egui::Key::Num5,
    egui::Key::Num6,
    egui::Key::Num7,
    egui::Key::Num8,
    egui::Key::Num9,
];

/// JSON 编辑器直接打开的 .json 文件大小上限。
const JSON_FILE_MAX_BYTES: u64 = 20 * 1024 * 1024;

/// 整个界面的状态，同时是 eframe 的应用对象（只在主线程上访问）。
pub struct HubApp {
    ctx: egui::Context,
    /// 与后台线程（剪贴板监听、快捷键、托盘、检查更新）共享的状态和事件队列。
    shared: Arc<Shared>,
    /// 界面线程自己的数据库连接（监听线程另开一个）。
    store: Store,
    data_dir: PathBuf,
    /// `--pasteboard` 指定的剪贴板名（测试时用私有剪贴板，不影响系统剪贴板）。
    pasteboard_name: Option<String>,
    settings: Settings,
    /// 全局快捷键服务；初始化失败时为 `None`，原因记在 `hotkey_errors`。
    hotkeys: Option<HotkeyService>,
    /// 各个全局快捷键（按 [`HotkeyAction::index`]）注册失败的原因。
    hotkey_errors: [Option<String>; 3],
    tray: Option<Tray>,
    page: Page,
    // 四个页面各自的界面状态。
    panel: PanelView,
    settings_view: SettingsView,
    quick: QuickPanelView,
    json: JsonEditorView,
    /// 鼠标中键监听线程是否已启动（只启动一次，之后由设置控制是否拦截）。
    middle_click_started: bool,
    /// 图片条目的缩略图纹理缓存。
    textures: Textures,
    /// 富文本代码的高亮样式。
    code: CodeStyles,
    /// 来源应用图标的纹理缓存。
    app_icons: AppIcons,
    toast: Option<Toast>,
    /// 是否已获得辅助功能权限（模拟按键需要，只有 macOS 会是 false）。
    accessibility_ok: bool,
    /// 上一帧窗口是否有焦点，用来发现失焦。
    was_focused: bool,
    /// 窗口的原生背景材质（离屏渲染测试中是不透明的）。
    backdrop: Backdrop,
    /// 栏的液态玻璃效果（需要 eframe 的 wgpu 渲染器，离屏测试里没有）。
    liquid: Option<LiquidGlass>,
    /// 第一帧之后需要把被 eframe 自动显示的窗口收起来。
    startup_hide_pending: bool,
    /// 在面板上关掉了提示的新版本（本次运行不再在面板上提示，菜单栏和设置页照常显示）。
    update_dismissed: Option<String>,
    /// 正在进行的截屏。
    snip: Option<Snipper>,
    /// 截屏刚结束、正在收起的遮罩（见 `closing_snip_ui`）。
    closing_snip: Option<screenshot::ClosingSnip>,
    /// 后台截屏线程的结果（截屏完成前不为空）。
    capturing: Option<std::sync::mpsc::Receiver<screenshot::CaptureResult>>,
    /// 截屏完成后要编辑的贴图（贴图的“编辑标注”）。
    pending_edit: Option<screenshot::PinEdit>,
    /// 屏幕上的贴图。
    pins: Vec<Pin>,
    next_pin_id: u64,
    /// 最近一次弹出的贴图右键菜单：（贴图编号，菜单）。
    pin_menu: Option<(u64, NativeMenu<PinAction>)>,
}

impl HubApp {
    /// 创建应用：打开数据库，接管原生窗口，注册全局快捷键，创建菜单栏 / 托盘图标。
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        shared: Arc<Shared>,
        data_dir: PathBuf,
        settings: Settings,
        args: &Args,
    ) -> Result<Self> {
        let store = Store::open(&data_dir).context("打开剪贴板数据库失败")?;
        let mut app = Self::build(
            cc.egui_ctx.clone(),
            shared.clone(),
            store,
            data_dir,
            settings,
        );
        app.pasteboard_name = args.pasteboard.clone();
        app.startup_hide_pending = !args.show && !args.after_update;
        if let Err(err) = native_panel::attach(cc, app.startup_hide_pending) {
            tracing::warn!("无法设置面板窗口：{err:#}");
        }
        app.backdrop = native_panel::backdrop();
        app.liquid = cc.wgpu_render_state.as_ref().map(LiquidGlass::new);
        match HotkeyService::new(shared.clone()) {
            Ok(service) => {
                app.hotkeys = Some(service);
                for action in HotkeyAction::ALL {
                    app.apply_hotkey(action);
                    if let Some(err) = &app.hotkey_errors[action.index()] {
                        tracing::warn!("{err}");
                    }
                }
            }
            Err(err) => {
                app.hotkey_errors[HotkeyAction::Panel.index()] =
                    Some(format!("无法初始化全局快捷键：{err}"));
            }
        }
        app.sync_middle_click();
        app.tray = Tray::new(shared.clone(), app.data_dir.clone(), &app.settings)
            .inspect_err(|err| tracing::warn!("无法创建菜单栏图标：{err:#}"))
            .ok();
        if args.after_update {
            // 应用内更新后重新启动：打开设置页底部，告诉用户已经是新版本（macOS 上可能还要重新授权）。
            controller::show_panel(&shared, true);
            shared.post(AppEvent::RevealUpdate);
            app.show_toast(format!("已更新到 {}", update::CURRENT_VERSION));
        } else if args.show {
            controller::show_panel(&shared, false);
        }
        if args.snip {
            shared.post(AppEvent::Snip);
        }
        if let Some(path) = &args.pin {
            match image::open(path) {
                Ok(image) => app.pin_at_cursor(image.to_rgba8()),
                Err(err) => tracing::warn!("无法打开图片 {}：{err}", path.display()),
            }
        }
        Ok(app)
    }

    /// 不涉及原生窗口、热键和菜单栏的部分（离屏渲染测试也用它）。
    fn build(
        ctx: egui::Context,
        shared: Arc<Shared>,
        store: Store,
        data_dir: PathBuf,
        settings: Settings,
    ) -> Self {
        style::install(&ctx);
        shared.set_ctx(ctx.clone());
        Self {
            ctx,
            shared,
            store,
            data_dir,
            pasteboard_name: None,
            settings,
            hotkeys: None,
            hotkey_errors: Default::default(),
            tray: None,
            page: Page::Panel,
            panel: PanelView::default(),
            settings_view: SettingsView::default(),
            quick: QuickPanelView::default(),
            json: JsonEditorView::default(),
            middle_click_started: false,
            textures: Textures::default(),
            code: CodeStyles::default(),
            app_icons: AppIcons::default(),
            toast: None,
            accessibility_ok: keyboard::accessibility_trusted(false),
            was_focused: false,
            backdrop: Backdrop::Opaque,
            liquid: None,
            startup_hide_pending: false,
            update_dismissed: None,
            snip: None,
            closing_snip: None,
            capturing: None,
            pending_edit: None,
            pins: Vec::new(),
            next_pin_id: 1,
            pin_menu: None,
        }
    }

    /// 按当前主题、窗口材质与透明度设置得到的配色。
    fn palette(&self, ctx: &egui::Context) -> Palette {
        Palette::new(
            ctx.theme() == egui::Theme::Dark,
            self.backdrop,
            self.settings.panel_transparency,
        )
    }

    /// 处理后台线程发来的事件；数据库有新记录时让面板重新加载。
    fn handle_events(&mut self) {
        for event in self.shared.drain_events() {
            match event {
                AppEvent::TogglePanel => controller::toggle_panel(&self.shared),
                AppEvent::PanelShown => self.on_shown(Page::Panel),
                AppEvent::OpenSettings => self.on_shown(Page::Settings),
                AppEvent::PauseChanged => {
                    self.settings.paused = self.shared.paused.load(Ordering::Relaxed);
                    self.persist_settings();
                }
                AppEvent::MiddleClick => self.on_middle_click(),
                AppEvent::UpdateChanged => {
                    self.sync_update_prompt();
                    update::restart_if_installed(&self.shared, &self.relaunch_args());
                }
                AppEvent::RevealUpdate => self.settings_view.reveal_update = true,
                AppEvent::MenuCommand(id) => {
                    if !self.pin_menu_command(&id) {
                        self.menu_command(&id);
                    }
                }
                AppEvent::Snip => self.start_snip(),
                AppEvent::SnipCaptured => self.finish_capture(),
                AppEvent::PinClipboard => self.pin_clipboard(),
                AppEvent::ClosePins => self.close_pins(),
                AppEvent::Pin(id, request) => self.pin_request(id, request),
            }
        }
        let revision = self.shared.revision.load(Ordering::Relaxed);
        if revision != self.panel.seen_revision {
            self.panel.seen_revision = revision;
            self.panel.needs_reload = true;
        }
    }

    /// 窗口被唤起（快捷键、托盘、菜单）后切到指定页面并重置面板状态。
    fn on_shown(&mut self, page: Page) {
        // 已经被主动显示，就不要再执行启动时的隐藏。
        self.startup_hide_pending = false;
        self.cancel_hotkey_recording();
        self.page = page;
        self.accessibility_ok = keyboard::accessibility_trusted(false);
        self.panel.reset_for_show();
        if page == Page::Settings {
            self.settings_view.refresh(&self.store);
        }
    }

    /// 隐藏面板，并记住用户调整过的窗口大小（PasteNow 存在 `float-panel-size` 偏好里）。
    /// 剪贴板面板与 JSON 编辑器分别记录；快捷面板的大小是固定的。
    fn hide_panel(&mut self, restore_focus: bool) {
        if let Some(rect) = self.ctx.input(|i| i.viewport().inner_rect) {
            let size = [rect.width().round(), rect.height().round()];
            let saved = match self.page {
                Page::Panel | Page::Settings => Some(&mut self.settings.window_size),
                Page::JsonEditor => Some(&mut self.settings.json_editor_size),
                Page::QuickActions => None,
            };
            if let Some(saved) = saved {
                if size != *saved && size[0] > 100.0 && size[1] > 100.0 {
                    *saved = size;
                    self.persist_settings();
                }
            }
        }
        native_panel::hide(restore_focus);
        self.cancel_hotkey_recording();
        self.page = Page::Panel;
    }

    /// 设置变化后：同步给后台线程和托盘菜单，并写入设置文件。
    fn persist_settings(&mut self) {
        self.shared.apply_settings(&self.settings);
        if let Some(tray) = &self.tray {
            tray.sync(&self.settings);
        }
        if let Err(err) = self.settings.save(&self.data_dir) {
            self.show_toast(format!("保存设置失败：{err:#}"));
        }
        self.sync_update_prompt();
    }

    /// 需要提醒的新版本（用户跳过的版本不提醒）。
    fn pending_update(&self) -> Option<Release> {
        let state = self.shared.update_state();
        state
            .available()
            .filter(|release| self.settings.skipped_version.as_ref() != Some(&release.version))
            .cloned()
    }

    /// 应用内更新后重新启动时传给新版本的参数：沿用同一个数据目录和剪贴板。
    fn relaunch_args(&self) -> Vec<std::ffi::OsString> {
        let mut args = vec!["--data-dir".into(), self.data_dir.clone().into_os_string()];
        if let Some(name) = &self.pasteboard_name {
            args.extend(["--pasteboard".into(), name.into()]);
        }
        args
    }

    /// 在当前窗口中打开设置页，并滚动到“分类标签”分组。
    fn reveal_tabs(&mut self) {
        self.settings_view.refresh(&self.store);
        self.page = Page::Settings;
        self.settings_view.reveal_tabs = true;
    }

    /// 在当前窗口中打开设置页，并滚动到底部的更新信息。
    fn reveal_update(&mut self) {
        self.settings_view.refresh(&self.store);
        self.page = Page::Settings;
        self.settings_view.reveal_update = true;
    }

    /// 同步菜单栏 / 托盘菜单里的新版本提示。
    fn sync_update_prompt(&mut self) {
        let pending = self.pending_update();
        if let Some(tray) = &mut self.tray {
            tray.set_update(pending.as_ref().map(|release| release.version.as_str()));
        }
    }

    /// 在窗口底部显示一条约两秒后消失的提示。
    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            until: Instant::now() + Duration::from_millis(2200),
        });
    }

    /// 要读写的剪贴板：系统剪贴板，或 `--pasteboard` 指定的私有剪贴板。
    fn pasteboard(&self) -> Pasteboard {
        self.pasteboard_name
            .as_deref()
            .map_or_else(Pasteboard::general, Pasteboard::named)
    }

    /// 选中条目后的粘贴流程：
    /// 写回剪贴板 → 隐藏面板，焦点回到之前的应用 → 稍等片刻发送 ⌘V。
    pub(crate) fn paste(&mut self, id: i64, mode: PasteMode) {
        let snapshot = match self.snapshot_for(id, mode) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                self.show_toast(format!("读取条目失败：{err:#}"));
                return;
            }
        };
        let pasteboard = self.pasteboard();
        self.shared.write_pasteboard(|| pasteboard.write(&snapshot));
        if let Err(err) = self
            .store
            .mark_pasted(id, self.settings.move_to_top_after_paste)
        {
            tracing::warn!("更新粘贴次数失败：{err:#}");
        }
        self.panel.needs_reload = true;

        if mode == PasteMode::CopyOnly || !self.settings.paste_to_active_app {
            self.hide_panel(true);
            return;
        }
        self.accessibility_ok = keyboard::accessibility_trusted(false);
        if !self.accessibility_ok {
            self.show_toast("已复制到剪贴板。自动粘贴需要“辅助功能”权限");
            // 只有 macOS 会走到这里：Windows 模拟按键不需要授权。
            return;
        }
        let keycode = keyboard::paste_keycode();
        self.hide_panel(true);
        std::thread::spawn(move || {
            // 等焦点回到之前的应用，否则 ⌘V 会发给错误的窗口。
            std::thread::sleep(Duration::from_millis(150));
            keyboard::post_paste(keycode);
        });
    }

    /// 鼠标中键：模拟复制读取选中的内容，然后在鼠标处弹出快捷面板。
    ///
    /// 与 uTools 不同，读取期间不先显示面板：面板显示时会拿走键盘焦点，复制快捷键就发不到原来的应用了。
    /// 读取一般只要几十毫秒，没有选中内容时最多等约 0.3 秒。
    fn on_middle_click(&mut self) {
        if !self.settings.middle_click || native_panel::is_active() {
            return;
        }
        self.startup_hide_pending = false;
        self.cancel_hotkey_recording();
        self.accessibility_ok = keyboard::accessibility_trusted(false);
        let selection = if self.accessibility_ok {
            let keycode = keyboard::copy_keycode();
            selection::capture(&self.shared, &self.pasteboard(), || {
                keyboard::post_copy(keycode)
            })
        } else {
            None
        };
        tracing::debug!(
            "鼠标中键：{}",
            selection
                .as_ref()
                .map_or_else(|| "没有选中内容".to_string(), Selection::label)
        );
        self.quick.open(selection);
        self.page = Page::QuickActions;
        native_panel::show(
            Anchor::Placement(Placement::NearCursor),
            self.quick.window_size(),
        );
    }

    /// 打开 JSON 编辑器：有选中内容时载入并自动格式化，没有时保留上次的内容。
    fn open_json_editor(&mut self, selection: Option<Selection>) {
        let indent = self.settings.json_indent;
        match selection {
            Some(Selection::Text(text)) => self.json.load(&text, indent),
            Some(Selection::Files(paths)) => {
                let path = std::path::Path::new(&paths[0]);
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                if size > JSON_FILE_MAX_BYTES {
                    self.show_toast(format!("文件太大（{}）", style::byte_size(size)));
                    self.json.focus();
                } else {
                    match std::fs::read_to_string(path) {
                        Ok(text) => self.json.load(&text, indent),
                        Err(err) => {
                            self.show_toast(format!("读取文件失败：{err}"));
                            self.json.focus();
                        }
                    }
                }
            }
            None => self.json.focus(),
        }
        self.page = Page::JsonEditor;
        native_panel::show(
            Anchor::Placement(Placement::ScreenCenter),
            self.settings.json_editor_size,
        );
    }

    /// 从快捷面板切换到剪贴板历史面板。
    fn open_clipboard_panel(&mut self) {
        self.on_shown(Page::Panel);
        native_panel::show(
            Anchor::Placement(self.settings.placement),
            self.settings.window_size,
        );
    }

    /// 设置开启时启动鼠标中键监听（只启动一次；关闭时监听线程直接放行中键）。
    fn sync_middle_click(&mut self) {
        if self.settings.middle_click && !self.middle_click_started {
            middle_click::start(self.shared.clone());
            self.middle_click_started = true;
        }
    }

    /// 粘贴时写回剪贴板的内容：纯文本粘贴只保留文字（文件条目是按行拼接的路径）。
    fn snapshot_for(&self, id: i64, mode: PasteMode) -> Result<Snapshot> {
        let snapshot = self.store.snapshot(id)?;
        if mode != PasteMode::PlainText {
            return Ok(snapshot);
        }
        let analysis = uti::analyze(&snapshot);
        let text = if analysis.file_paths.is_empty() {
            uti::plain_text(&snapshot).unwrap_or_default()
        } else {
            analysis.file_paths.join("\n")
        };
        Ok(if text.is_empty() {
            snapshot
        } else {
            Snapshot::from_text(&text)
        })
    }

    /// 窗口失去焦点时按页面与设置决定是否自动收起。
    fn handle_focus(&mut self) {
        // Windows 上 winit 的焦点状态在托盘弹出路径下不可靠：面板已经是前台窗口，
        // 但 AttachThreadInput 的 detach 会撤销键盘焦点，has_focus() 恒为 false。
        let focused = if cfg!(windows) {
            native_panel::is_active()
        } else {
            self.ctx.input(|i| i.viewport().focused).unwrap_or(false)
        };
        let lost_focus = self.was_focused && !focused;
        self.was_focused = focused;
        let auto_hide = match self.page {
            Page::Panel => self.settings.hide_on_blur && !self.panel.pinned,
            Page::Settings => false,
            // 快捷面板与 uTools 一样，点到别处就收起。
            Page::QuickActions => true,
            Page::JsonEditor => self.settings.hide_on_blur && !self.json.pinned,
        };
        if lost_focus && auto_hide && native_panel::is_visible() {
            // 用户点击了别的应用：只隐藏，不抢回焦点。
            controller::note_auto_hide();
            self.hide_panel(false);
        }
    }

    /// 绘制提示，到期后清除。
    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some(toast) = &self.toast else {
            return;
        };
        let now = Instant::now();
        if now >= toast.until {
            self.toast = None;
            return;
        }
        // JSON 编辑器底部有两行工具栏，提示放在它上面。
        let bottom = if self.page == Page::JsonEditor {
            -96.0
        } else {
            -48.0
        };
        egui::Area::new(egui::Id::new("toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, bottom))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(200))
                    .corner_radius(style::CAPSULE)
                    .inner_margin(egui::Margin::symmetric(14, 7))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&toast.text)
                                .color(egui::Color32::WHITE)
                                .size(12.5),
                        );
                    });
            });
        ctx.request_repaint_after(toast.until - now);
    }
}

impl eframe::App for HubApp {
    /// 每帧绘制前先处理后台线程发来的事件。
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.check_snip_rescued();
        self.handle_events();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.startup_hide_pending {
            // eframe 在第一帧渲染后调用 set_visible(true)，所以在第二帧收起窗口。
            if ui.ctx().cumulative_frame_nr() > 0 {
                native_panel::finish_startup_hide();
                self.startup_hide_pending = false;
            } else {
                ui.ctx().request_repaint();
            }
        }
        self.handle_focus();
        // Windows 上面板显示期间定时刷新：失焦不一定产生 repaint 事件（winit 的焦点状态可能已经不一致），
        // 需要轮询才能发现焦点变化。macOS 的失焦事件可靠，不需要轮询；隐藏时不排重绘，保持空闲。
        if cfg!(windows) && self.page != Page::Settings && native_panel::is_visible() {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        // 截屏遮罩或贴图在显示时，面板隐藏着 eframe 也会运行界面，这时不用绘制面板。
        if native_panel::is_visible() {
            self.draw(ui);
        }
        let ctx = ui.ctx().clone();
        self.closing_snip_ui(&ctx);
        self.snip_ui(&ctx);
        self.pins_ui(&ctx);
        // 为创建截屏遮罩、贴图而临时摆出的面板（见 `panel::wake`）完成了任务，收起来。
        native_panel::finish_wake();
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.backdrop != Backdrop::Opaque {
            // 窗口透明（macOS 毛玻璃 / 液态玻璃），圆角背景在 `draw` 里绘制。
            [0.0; 4]
        } else {
            Palette::new(visuals.dark_mode, Backdrop::Opaque, 0)
                .background
                .to_normalized_gamma_f32()
        }
    }
}

impl HubApp {
    /// 绘制圆角背景与当前页面，最后叠上提示。
    fn draw(&mut self, ui: &mut egui::Ui) {
        let palette = self.palette(ui.ctx());
        let inner_margin = match self.page {
            Page::Panel | Page::JsonEditor => egui::Margin::ZERO,
            Page::Settings => egui::Margin::same(style::PAD),
            Page::QuickActions => egui::Margin::same(quick_panel::MARGIN as i8),
        };
        // 液态玻璃自带高光描边，不再叠一圈边框。
        let border = if self.backdrop == Backdrop::Glass {
            egui::Stroke::NONE
        } else {
            egui::Stroke::new(1.0, palette.border)
        };
        // 背景与系统窗口一样用连续曲率的圆角，与下面的液态玻璃 / 毛玻璃形状一致。
        ui.painter().add(style::continuous_rect(
            ui.max_rect(),
            native_panel::CORNER_RADIUS as f32,
            palette.background,
            border,
        ));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(inner_margin))
            .show(ui, |ui| match self.page {
                Page::Panel => self.panel_ui(ui),
                Page::Settings => self.settings_ui(ui),
                Page::QuickActions => self.quick_ui(ui),
                Page::JsonEditor => self.json_editor_ui(ui),
            });
        let ctx = self.ctx.clone();
        self.toast_ui(&ctx);
    }
}
