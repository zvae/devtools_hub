//! egui 界面：剪贴板面板、设置页、鼠标中键快捷面板与 JSON 编辑器。
//!
//! 四个页面共用同一个窗口：切换页面时由平台层调整窗口大小和位置。

mod code_text;
mod fonts;
mod glass;
mod icons;
mod json_editor;
mod liquid_glass;
mod menu;
mod native_menu;
mod panel;
mod quick_panel;
mod settings_page;
#[cfg(test)]
mod snapshot_tests;
mod style;
mod textures;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use devtools_clipboard::{uti, Snapshot, Store};

use crate::hotkey::{self, HotkeyService};
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
use panel::PanelView;
use quick_panel::QuickPanelView;
use settings_page::SettingsView;
use style::Palette;
use textures::{AppIcons, Textures};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
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

struct Toast {
    text: String,
    until: Instant,
}

/// JSON 编辑器直接打开的 .json 文件大小上限。
const JSON_FILE_MAX_BYTES: u64 = 20 * 1024 * 1024;

pub struct HubApp {
    ctx: egui::Context,
    shared: Arc<Shared>,
    store: Store,
    data_dir: PathBuf,
    pasteboard_name: Option<String>,
    settings: Settings,
    hotkeys: Option<HotkeyService>,
    hotkey_error: Option<String>,
    tray: Option<Tray>,
    page: Page,
    panel: PanelView,
    settings_view: SettingsView,
    quick: QuickPanelView,
    json: JsonEditorView,
    /// 鼠标中键监听线程是否已启动（只启动一次，之后由设置控制是否拦截）。
    middle_click_started: bool,
    textures: Textures,
    /// 富文本代码的高亮样式。
    code: CodeStyles,
    app_icons: AppIcons,
    toast: Option<Toast>,
    accessibility_ok: bool,
    was_focused: bool,
    /// 窗口的原生背景材质（离屏渲染测试中是不透明的）。
    backdrop: Backdrop,
    /// 栏的液态玻璃效果（需要 eframe 的 wgpu 渲染器，离屏测试里没有）。
    liquid: Option<LiquidGlass>,
    /// 第一帧之后需要把被 eframe 自动显示的窗口收起来。
    startup_hide_pending: bool,
    /// 在面板上关掉了提示的新版本（本次运行不再在面板上提示，菜单栏和设置页照常显示）。
    update_dismissed: Option<String>,
}

impl HubApp {
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
            Ok(mut service) => {
                if let Err(err) = service.apply(&app.settings.hotkey) {
                    tracing::warn!("{err}");
                    app.hotkey_error = Some(err);
                }
                app.hotkeys = Some(service);
            }
            Err(err) => app.hotkey_error = Some(format!("无法初始化全局快捷键：{err}")),
        }
        app.sync_middle_click();
        app.tray = Tray::new(
            shared.clone(),
            app.data_dir.clone(),
            &hotkey::display(&app.settings.hotkey),
        )
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
            hotkey_error: None,
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
        }
    }

    fn palette(&self, ctx: &egui::Context) -> Palette {
        Palette::new(
            ctx.theme() == egui::Theme::Dark,
            self.backdrop,
            self.settings.panel_transparency,
        )
    }

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
                AppEvent::MenuCommand(id) => self.menu_command(&id),
            }
        }
        let revision = self.shared.revision.load(Ordering::Relaxed);
        if revision != self.panel.seen_revision {
            self.panel.seen_revision = revision;
            self.panel.needs_reload = true;
        }
    }

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

    fn persist_settings(&mut self) {
        self.shared.apply_settings(&self.settings);
        if let Some(tray) = &self.tray {
            tray.sync(
                self.settings.paused,
                &hotkey::display(&self.settings.hotkey),
            );
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

    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            until: Instant::now() + Duration::from_millis(2200),
        });
    }

    fn pasteboard(&self) -> Pasteboard {
        self.pasteboard_name
            .as_deref()
            .map_or_else(Pasteboard::general, Pasteboard::named)
    }

    /// 选中条目后的粘贴流程：
    /// 写回剪贴板 → 隐藏面板并激活之前的应用 → 稍等片刻发送 ⌘V。
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
            // 等之前的应用重新成为前台，否则 ⌘V 会发给错误的窗口。
            std::thread::sleep(Duration::from_millis(150));
            keyboard::post_paste(keycode);
        });
    }

    /// 鼠标中键：模拟复制读取选中的内容，然后在鼠标处弹出快捷面板。
    ///
    /// 与 uTools 不同，读取期间不先显示面板：面板显示时会激活本应用，复制快捷键就发不到原来的应用了。
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
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
        self.draw(ui);
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
