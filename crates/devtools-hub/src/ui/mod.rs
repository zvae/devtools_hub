//! egui 界面：剪贴板面板、设置页、鼠标中键快捷面板、JSON 编辑器与提取文字，以及截屏遮罩和贴图窗口。
//!
//! 这几个页面共用同一个窗口：切换页面时由平台层调整窗口大小和位置。
//! 截屏遮罩（`snipper`）和贴图（`pin`）是各自独立的窗口（egui 的子视口），流程在 `screenshot` 里。

mod code_text;
mod fonts;
mod glass;
mod icons;
mod json_editor;
mod liquid_glass;
mod menu;
mod native_menu;
mod ocr;
mod panel;
mod pin;
mod quick_panel;
mod screenshot;
mod search_host;
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
use crate::platform::{floating, keyboard, panel as native_panel, Anchor, Backdrop, Placement};
use crate::selection::{self, Selection};
use crate::settings::Settings;
use crate::shared::{AppEvent, SearchEntry, Shared};
use crate::tray::Tray;
use crate::update::{self, Release};
use crate::{controller, middle_click, Args};

use devtools_search::{SearchModule, SearchOptions};

use search_host::HostBridge;

use code_text::CodeStyles;
use json_editor::JsonEditorView;
use liquid_glass::LiquidGlass;
use native_menu::NativeMenu;
use ocr::OcrView;
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
    /// 提取文字（截屏、贴图的识别结果）。
    Ocr,
    /// 文件搜索页（不透明底、共享窗口，§5.2）。
    Search,
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
    hotkey_errors: [Option<String>; 4],
    tray: Option<Tray>,
    page: Page,
    // 各个页面的界面状态。
    panel: PanelView,
    settings_view: SettingsView,
    quick: QuickPanelView,
    json: JsonEditorView,
    ocr: OcrView,
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
    /// 刚刚关掉系统的保存对话框：下一次失焦不要据此自动收起
    /// （对话框收尾时的焦点变化不是用户点了别的应用，见 `floating::save_dialog`）。
    just_closed_dialog: bool,
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
    /// 文件搜索模块（开关关闭时为 `None`，§4.4）。
    search: Option<SearchModule>,
    /// 宿主的平台能力实现（文件夹选择、模态保护、窗口拖动等）。
    search_host: Arc<dyn devtools_search::HostServices>,
    /// 下次打开搜索页要传入的入口信息。
    search_entry: Option<SearchOptions>,
    /// 正在等待搜索模块释放（关闭开关后）。
    search_stopping: bool,
    /// 本帧绘制结束时收起窗口（Esc）。
    pending_hide: bool,
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
        app.sync_search_module();
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
        let search_host = HostBridge::new(shared.clone());
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
            ocr: OcrView::default(),
            middle_click_started: false,
            textures: Textures::default(),
            code: CodeStyles::default(),
            app_icons: AppIcons::default(),
            toast: None,
            accessibility_ok: keyboard::accessibility_trusted(false),
            was_focused: false,
            just_closed_dialog: false,
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
            search: None,
            search_host,
            search_entry: None,
            search_stopping: false,
            pending_hide: false,
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
                AppEvent::OcrDone => self.ocr.poll(),
                AppEvent::HidePanel => self.hide_panel(true),
                AppEvent::ShowSearch(entry) => self.open_search(entry),
                // 托盘"退出"：走正常关闭流程，让 on_exit 保存尺寸并停止索引（§4.6）。
                AppEvent::Quit => self.ctx.send_viewport_cmd(egui::ViewportCommand::Close),
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
        self.set_page(page);
        self.accessibility_ok = keyboard::accessibility_trusted(false);
        self.panel.reset_for_show();
        if page == Page::Settings {
            self.settings_view.refresh(&self.store);
        }
    }

    /// 统一切换页面（§4.4：所有 `self.page = ...` 都必须经过这里）。
    ///
    /// 离开搜索页时完成尺寸保存、空格行为复位与最小尺寸恢复；进入时聚焦输入框。
    fn set_page(&mut self, to: Page) {
        let from = self.page;
        if from == to {
            return;
        }
        self.page = to;
        if from == Page::Search {
            if let Some(search) = &mut self.search {
                search.on_hidden();
            }
            self.apply_min_size(false);
        }
        if to == Page::Search {
            if let Some(search) = &mut self.search {
                search.warm_up();
                search.on_shown(self.search_entry.take());
            }
            self.apply_min_size(true);
        }
    }

    /// 搜索页需要 800×480 起，其他页面沿用窗口原来的最小尺寸。
    fn apply_min_size(&self, searching: bool) {
        let min = if searching {
            egui::vec2(800.0, 480.0)
        } else {
            egui::vec2(240.0, 120.0)
        };
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::MinInnerSize(min));
    }

    /// 打开 / 收起搜索页（热键、托盘、快捷面板都走这里，§4.5）。
    fn open_search(&mut self, entry: SearchEntry) {
        self.sync_search_module();
        if !self.settings.search_enabled {
            self.show_toast("文件搜索未启用，可在设置的“文件搜索”中开启");
            return;
        }
        if self.search.is_none() {
            return;
        }
        // 已经在前台：热键再次按下时收起。
        if self.page == Page::Search && native_panel::is_active() {
            self.hide_panel(true);
            return;
        }
        self.search_entry = match entry {
            SearchEntry::Hotkey => None,
            SearchEntry::Text(text) => Some(SearchOptions {
                query: text,
                selected_paths: Vec::new(),
                query_is_literal: true,
                case_sensitive: self.settings.search_enabled
                    && self
                        .search
                        .as_ref()
                        .map(|s| s.service().settings().default_case_sensitive)
                        .unwrap_or(false),
            }),
            SearchEntry::Paths(paths) => Some(SearchOptions {
                query: String::new(),
                selected_paths: paths.into_iter().map(PathBuf::from).collect(),
                query_is_literal: false,
                case_sensitive: false,
            }),
        };
        self.startup_hide_pending = false;
        self.on_shown(Page::Search);
        let size = self
            .search
            .as_ref()
            .map(SearchModule::window_size)
            .unwrap_or([1100.0, 680.0]);
        native_panel::show(Anchor::Placement(self.settings.placement), size);
    }

    /// 按开关创建 / 释放搜索模块（关闭时不再占线程，§4.4）。
    fn sync_search_module(&mut self) {
        if self.settings.search_enabled {
            if self.search.is_none() && !self.search_stopping {
                let mut module = SearchModule::new(&self.data_dir, self.search_host.clone());
                module.warm_up();
                self.search = Some(module);
            }
        } else if let Some(module) = &mut self.search {
            module.shutdown();
            self.search_stopping = true;
        }
        self.apply_hotkey(crate::hotkey::HotkeyAction::Search);
    }

    /// 拉取搜索模块的事件、响应策略变化、回收已停止的模块。
    fn poll_search(&mut self) {
        let mut policy_changed = false;
        if let Some(search) = &mut self.search {
            policy_changed = search.poll().policy_changed;
        }
        if policy_changed && self.page == Page::Search {
            self.apply_min_size(true);
        }
        if self.search_stopping && self.search.as_ref().is_some_and(SearchModule::is_stopped) {
            self.search = None;
            self.search_stopping = false;
            if self.page == Page::Search {
                self.set_page(Page::Panel);
            }
        }
    }

    /// 隐藏面板，并记住用户调整过的窗口大小（PasteNow 存在 `float-panel-size` 偏好里）。
    /// 剪贴板面板、JSON 编辑器与提取文字分别记录；快捷面板的大小是固定的。
    fn hide_panel(&mut self, restore_focus: bool) {
        if let Some(rect) = self.ctx.input(|i| i.viewport().inner_rect) {
            let size = [rect.width().round(), rect.height().round()];
            let saved = match self.page {
                Page::Panel | Page::Settings => Some(&mut self.settings.window_size),
                Page::JsonEditor => Some(&mut self.settings.json_editor_size),
                Page::Ocr => Some(&mut self.settings.ocr_size),
                Page::QuickActions | Page::Search => None,
            };
            if let Some(saved) = saved {
                if size != *saved && size[0] > 100.0 && size[1] > 100.0 {
                    *saved = size;
                    self.persist_settings();
                }
            }
            // 搜索页的尺寸由模块自己记住（§4.1 remember_size 无参，尺寸走这里）。
            if self.page == Page::Search {
                if let Some(search) = &mut self.search {
                    search.remember_size(size);
                }
            }
        }
        self.set_page(Page::Panel);
        native_panel::hide(restore_focus);
        self.cancel_hotkey_recording();
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
        // 文件搜索开关变化时创建/释放模块并重新注册热键（§4.4）。
        self.sync_search_module();
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
        self.set_page(Page::Settings);
        self.settings_view.reveal_tabs = true;
    }

    /// 在当前窗口中打开设置页，并滚动到底部的更新信息。
    fn reveal_update(&mut self) {
        self.settings_view.refresh(&self.store);
        self.set_page(Page::Settings);
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
        self.quick.open(selection, self.settings.search_enabled);
        self.set_page(Page::QuickActions);
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
        self.set_page(Page::JsonEditor);
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
        // 这一次失焦是不是对话框收尾造成的（见字段说明）。
        let just_closed_dialog = std::mem::replace(&mut self.just_closed_dialog, false);
        let auto_hide = match self.page {
            Page::Panel => self.settings.hide_on_blur && !self.panel.pinned,
            Page::Settings => false,
            // 快捷面板与 uTools 一样，点到别处就收起。
            Page::QuickActions => true,
            Page::JsonEditor => self.settings.hide_on_blur && !self.json.pinned,
            Page::Ocr => self.settings.hide_on_blur && !self.ocr.pinned,
            // 搜索页按窗口策略决定（Floating 失焦收起，Pinned 保留，§5.2）。
            Page::Search => self
                .search
                .as_ref()
                .is_some_and(|search| search.window_policy().hide_on_blur),
        };
        // 系统对话框（保存图片）开着的时候失焦是正常的，关掉之后的第一下失焦也是它收尾造成的：
        // 都不是用户点了别的应用，不能收起窗口。
        if lost_focus
            && auto_hide
            && !just_closed_dialog
            && !floating::dialog_open()
            && native_panel::is_visible()
        {
            // 用户点击了别的应用：只隐藏，不抢回焦点。
            tracing::info!("面板失去焦点，自动收起");
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
        // JSON 编辑器底部有两行工具栏、提取文字有一行，提示放在它们上面。
        let bottom = match self.page {
            Page::JsonEditor => -96.0,
            Page::Ocr => -64.0,
            _ => -48.0,
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
        self.poll_search();
    }

    /// 退出前：保存搜索页尺寸并停止索引（§4.6：托盘退出、更新重启都走这里）。
    fn on_exit(&mut self) {
        if let Some(rect) = self.ctx.input(|i| i.viewport().inner_rect) {
            if self.page == Page::Search {
                if let Some(search) = &mut self.search {
                    search.remember_size([rect.width().round(), rect.height().round()]);
                }
            }
        }
        if let Some(mut search) = self.search.take() {
            // 有界阻塞：取消后等待线程池收尾，最多 1.5 秒。
            if !search.shutdown_blocking(Duration::from_millis(1500)) {
                tracing::warn!("搜索模块未在 1.5 秒内停止，继续退出");
            }
        }
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
        // 隐藏着创建的遮罩还没显示时除外：面板一收起，eframe 就只调用 `logic`，没有人去显示遮罩了。
        if !self
            .snip
            .as_ref()
            .is_some_and(|snip| snip.waiting_to_show())
        {
            native_panel::finish_wake();
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        // 搜索页不透明（§5.2）：清屏色与内容底色一致，透明窗口边缘不会透出来。
        if self.page == Page::Search {
            return devtools_search::ui::page_background(visuals.dark_mode)
                .to_normalized_gamma_f32();
        }
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
            Page::Panel | Page::JsonEditor | Page::Ocr | Page::Search => egui::Margin::ZERO,
            Page::Settings => egui::Margin::same(style::PAD),
            Page::QuickActions => egui::Margin::same(quick_panel::MARGIN as i8),
        };
        // 液态玻璃自带高光描边，不再叠一圈边框。
        let border = if self.backdrop == Backdrop::Glass {
            egui::Stroke::NONE
        } else {
            egui::Stroke::new(1.0, palette.border)
        };
        // 搜索页是不透明底（§5.2）：与模块使用同一份底色，圆角窗口与内容不会出现色差。
        let background = if self.page == Page::Search {
            devtools_search::ui::page_background(ui.visuals().dark_mode)
        } else {
            palette.background
        };
        // 背景与系统窗口一样用连续曲率的圆角，与下面的液态玻璃 / 毛玻璃形状一致。
        ui.painter().add(style::continuous_rect(
            ui.max_rect(),
            native_panel::CORNER_RADIUS as f32,
            background,
            border,
        ));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(inner_margin))
            .show(ui, |ui| match self.page {
                Page::Panel => self.panel_ui(ui),
                Page::Settings => self.settings_ui(ui),
                Page::QuickActions => self.quick_ui(ui),
                Page::JsonEditor => self.json_editor_ui(ui),
                Page::Ocr => self.ocr_ui(ui),
                Page::Search => self.search_ui(ui),
            });
        if std::mem::take(&mut self.pending_hide) {
            self.hide_panel(true);
        }
        let ctx = self.ctx.clone();
        self.toast_ui(&ctx);
    }

    /// 搜索页：模块负责内容，宿主只负责背景与窗口（§4.2）。
    fn search_ui(&mut self, ui: &mut egui::Ui) {
        let Some(search) = &mut self.search else {
            return;
        };
        let output = search.ui(ui);
        if output.close {
            self.pending_hide = true;
        }
    }
}
