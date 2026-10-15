//! egui 界面：剪贴板面板与设置页。

mod fonts;
mod icons;
mod panel;
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
use crate::platform::{keyboard, panel as native_panel};
use crate::settings::Settings;
use crate::shared::{AppEvent, Shared};
use crate::tray::Tray;
use crate::{controller, Args};

use panel::PanelView;
use settings_page::SettingsView;
use style::Palette;
use textures::{AppIcons, Textures};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Panel,
    Settings,
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
    textures: Textures,
    app_icons: AppIcons,
    toast: Option<Toast>,
    accessibility_ok: bool,
    was_focused: bool,
    /// 窗口是否有原生毛玻璃背景（离屏渲染测试中没有）。
    translucent: bool,
    /// 第一帧之后需要把被 eframe 自动显示的窗口收起来。
    startup_hide_pending: bool,
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
        app.startup_hide_pending = !args.show;
        if let Err(err) = native_panel::attach(cc, app.startup_hide_pending) {
            tracing::warn!("无法设置面板窗口：{err:#}");
        }
        app.translucent = native_panel::has_vibrancy();
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
        app.tray = Tray::new(
            shared.clone(),
            app.data_dir.clone(),
            &hotkey::display(&app.settings.hotkey),
        )
        .inspect_err(|err| tracing::warn!("无法创建菜单栏图标：{err:#}"))
        .ok();
        if args.show {
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
            textures: Textures::default(),
            app_icons: AppIcons::default(),
            toast: None,
            accessibility_ok: keyboard::accessibility_trusted(false),
            was_focused: false,
            translucent: false,
            startup_hide_pending: false,
        }
    }

    fn palette(&self, ctx: &egui::Context) -> Palette {
        Palette::new(ctx.theme() == egui::Theme::Dark, self.translucent)
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
    fn hide_panel(&mut self, restore_focus: bool) {
        if let Some(rect) = self.ctx.input(|i| i.viewport().inner_rect) {
            let size = [rect.width().round(), rect.height().round()];
            if size != self.settings.window_size && size[0] > 100.0 && size[1] > 100.0 {
                self.settings.window_size = size;
                self.persist_settings();
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
        let focused = self.ctx.input(|i| i.viewport().focused).unwrap_or(false);
        let lost_focus = self.was_focused && !focused;
        self.was_focused = focused;
        if lost_focus
            && self.settings.hide_on_blur
            && !self.panel.pinned
            && self.page == Page::Panel
            && native_panel::is_visible()
        {
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
        egui::Area::new(egui::Id::new("toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(200))
                    .corner_radius(14)
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
        self.draw(ui);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.translucent {
            // 窗口透明（macOS 毛玻璃），圆角背景由 CentralPanel 的 Frame 绘制。
            [0.0; 4]
        } else {
            Palette::new(visuals.dark_mode, false)
                .background
                .to_normalized_gamma_f32()
        }
    }
}

impl HubApp {
    fn draw(&mut self, ui: &mut egui::Ui) {
        let palette = self.palette(ui.ctx());
        let inner_margin = match self.page {
            Page::Panel => egui::Margin::ZERO,
            Page::Settings => egui::Margin::same(14),
        };
        let frame = egui::Frame::new()
            .fill(palette.background)
            .stroke(egui::Stroke::new(1.0, palette.border))
            .corner_radius(native_panel::CORNER_RADIUS as u8)
            .inner_margin(inner_margin);
        egui::CentralPanel::default()
            .frame(frame)
            .show(ui, |ui| match self.page {
                Page::Panel => self.panel_ui(ui),
                Page::Settings => self.settings_ui(ui),
            });
        let ctx = self.ctx.clone();
        self.toast_ui(&ctx);
    }
}
