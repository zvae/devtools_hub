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
mod search_window;
mod settings_page;
#[cfg(test)]
mod snapshot_tests;
mod snipper;
mod style;
mod textures;
mod widgets;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
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

/// 搜索模块的共享句柄：宿主和搜索窗口的延迟视口回调都会访问它。
type SearchHandle = Arc<Mutex<SearchModule>>;

/// 取搜索模块的锁；某个回调 panic 污染了锁也继续使用，避免整个搜索窗口不可用。
fn lock_search(search: &SearchHandle) -> MutexGuard<'_, SearchModule> {
    search.lock().unwrap_or_else(|poison| poison.into_inner())
}

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
    /// 文件搜索页。生产环境搜索页是独立窗口（见 `search_window`），
    /// 这个页面只保留给离屏渲染测试在共享窗口里直接绘制搜索界面。
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
    ///
    /// 用 `Arc<Mutex<..>>` 共享：搜索窗口是延迟视口，它的回调会在独立的重绘里访问模块，
    /// 宿主（索引状态、设置、开关）也在主线程访问同一份状态。
    search: Option<SearchHandle>,
    /// 宿主的平台能力实现（文件夹选择、模态保护、窗口拖动等）。
    search_host: Arc<dyn devtools_search::HostServices>,
    /// 下次打开搜索页要传入的入口信息。
    search_entry: Option<SearchOptions>,
    /// 搜索页的独立窗口是否已经打开（与剪贴板面板的页面互不影响）。
    search_open: bool,
    /// 搜索窗口里的 Esc / 关闭按钮 / 浮动模式失焦：由视口回调置位，宿主在下一帧收起。
    search_close_requested: Arc<AtomicBool>,
    /// 上一帧搜索窗口是否有焦点（浮动模式下用它发现失焦，视口回调和宿主共用）。
    search_focus_was: Arc<AtomicBool>,
    /// 刚切换过窗口模式的宽限期：切换激活策略可能造成一次瞬时的失焦，这段时间内不收起。
    search_blur_grace_until: Arc<Mutex<Option<Instant>>>,
    /// 搜索窗口创建后需要主动聚焦一次（系统激活可能晚于窗口创建）。
    search_focus_pending: bool,
    /// 打开搜索窗口时使用的稳定尺寸（用户缩放后不逐帧回写，避免和拖动打架）。
    search_window_size: [f32; 2],
    /// 搜索窗口在本次打开期间的位置（模式切换重建窗口时保持不变）。
    search_window_pos: egui::Pos2,
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
        #[cfg(target_os = "macos")]
        native_panel::install_application_icon();
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
            search_open: false,
            search_close_requested: Arc::new(AtomicBool::new(false)),
            search_focus_was: Arc::new(AtomicBool::new(false)),
            search_blur_grace_until: Arc::new(Mutex::new(None)),
            search_focus_pending: false,
            search_window_size: [1100.0, 680.0],
            search_window_pos: egui::Pos2::ZERO,
            search_stopping: false,
            pending_hide: false,
        }
    }

    /// 按当前主题、窗口材质与透明度设置得到的配色。
    fn palette(&self, ctx: &egui::Context) -> Palette {
        // 窗口背景是系统材质（毛玻璃 / 液态玻璃）时底色半透明，让背后的材质透出来；
        // 不透明窗口用满 alpha。透明度设置只在材质背景下生效。
        let transparency = self.settings.panel_transparency;
        let alpha = match self.backdrop {
            Backdrop::Opaque => 255,
            Backdrop::Vibrancy | Backdrop::Glass => {
                (255.0 * (1.0 - f32::from(transparency.min(100)) / 100.0)).round() as u8
            }
        };
        Palette::with_alpha(ctx.theme() == egui::Theme::Dark, alpha)
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
                AppEvent::Quit => {
                    tracing::info!("退出：发送窗口关闭命令");
                    // 告诉托盘退出看门狗：Quit 已消费，不用强杀。
                    self.shared.quit_consumed.store(true, Ordering::Relaxed);
                    crate::shared::quit_trace("2 ui: Quit event -> ViewportCommand::Close");
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
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
            if let Some(handle) = &self.search {
                lock_search(handle).on_hidden();
            }
            self.apply_min_size(false);
        }
        if to == Page::Search {
            if let Some(handle) = &self.search {
                let mut search = lock_search(handle);
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

    /// 打开 / 收起搜索窗口（热键、托盘、快捷面板都走这里，§4.5）。
    ///
    /// 搜索页是独立窗口：已经打开时热键只负责前置 / 收起，不再切换共享窗口的页面。
    fn open_search(&mut self, entry: SearchEntry) {
        self.sync_search_module();
        if !self.settings.search_enabled {
            self.show_toast("文件搜索未启用，可在设置的“文件搜索”中开启");
            return;
        }
        let Some(handle) = self.search.as_ref().cloned() else {
            return;
        };
        let options = match entry {
            SearchEntry::Hotkey => None,
            SearchEntry::Text(text) => Some(SearchOptions {
                query: text,
                selected_paths: Vec::new(),
                query_is_literal: true,
                case_sensitive: self.settings.search_enabled
                    && lock_search(&handle)
                        .service()
                        .settings()
                        .default_case_sensitive,
            }),
            SearchEntry::Paths(paths) => Some(SearchOptions {
                query: String::new(),
                selected_paths: paths.into_iter().map(PathBuf::from).collect(),
                query_is_literal: false,
                case_sensitive: false,
            }),
        };
        if self.search_open {
            let info = self
                .ctx
                .input(|i| i.raw.viewports.get(&search_window::viewport()).cloned());
            let Some(info) = info else {
                // 窗口已经随上一帧关闭，这里只清掉状态。
                self.close_search();
                return;
            };
            let minimized = info.minimized.unwrap_or(false);
            let focused = info.focused.unwrap_or(false);
            if !minimized && focused && options.is_none() {
                self.close_search();
                return;
            }
            // 打开了新入口（例如选中文件后再次唤起）：更新查询 / 范围并前置。
            if options.is_some() {
                lock_search(&handle).on_shown(options);
            }
            self.focus_search_window(minimized);
            return;
        }
        // 打开搜索时先收起剪贴板 / 快捷面板：搜索是普通窗口，面板是临时浮层。
        if native_panel::is_visible() {
            native_panel::hide(false);
        }
        // 已经被主动显示，就不要再执行启动时的隐藏。
        self.startup_hide_pending = false;
        self.search_open = true;
        self.search_close_requested.store(false, Ordering::Relaxed);
        self.search_focus_was.store(false, Ordering::Relaxed);
        *self
            .search_blur_grace_until
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        self.search_focus_pending = true;
        self.search_window_size = lock_search(&handle).window_size();
        // 出现在鼠标所在屏幕的中央；会话内保持这个位置（模式切换重建窗口也不跳）。
        self.search_window_pos = search_window::centered_position(
            self.search_window_size,
            crate::platform::screen::cursor().map(|(_, display)| display.logical),
        );
        {
            let mut search = lock_search(&handle);
            search.warm_up();
            search.on_shown(options);
        }
        // 面板隐藏时 eframe 只调用 logic；让面板以隐藏状态运行一帧界面，
        // 延迟视口（搜索窗口）才能被登记并创建（见 `panel::wake`）。
        native_panel::wake();
        native_panel::activate_search_window();
        self.apply_search_dock_visibility();
        self.ctx.request_repaint();
    }

    /// 关闭搜索窗口：保存尺寸、通知模块，并按需收起为创建子视口临时唤醒的面板。
    fn close_search(&mut self) {
        let was_open = self.search_open;
        self.search_open = false;
        self.search_focus_pending = false;
        self.search_focus_was.store(false, Ordering::Relaxed);
        *self
            .search_blur_grace_until
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        self.search_close_requested.store(false, Ordering::Relaxed);
        if let Some(handle) = self.search.as_ref() {
            let mut search = lock_search(handle);
            if let Some(rect) = self.ctx.input(|i| {
                i.raw
                    .viewports
                    .get(&search_window::viewport())
                    .and_then(|v| v.inner_rect)
            }) {
                if rect.width() > 100.0 && rect.height() > 100.0 {
                    search.remember_size([rect.width().round(), rect.height().round()]);
                }
            }
            if was_open {
                search.on_hidden();
            }
        }
        // 面板是隐藏的（可能正为创建子视口而临时唤醒）：把它真正收起来。
        // 面板本身可见时说明用户正在用剪贴板，不要跟着隐藏。
        if !native_panel::is_visible() {
            native_panel::finish_wake();
        }
        self.apply_search_dock_visibility();
    }

    /// 把搜索窗口前置；最小化时先恢复。
    fn focus_search_window(&mut self, minimized: bool) {
        let id = search_window::viewport();
        if minimized {
            self.ctx
                .send_viewport_cmd_to(id, egui::ViewportCommand::Minimized(false));
        }
        self.ctx
            .send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
    }

    /// 登记搜索窗口的延迟视口。
    ///
    /// 用延迟（而不是立即）视口：立即视口必须在父窗口的界面回调里同步创建原生窗口，
    /// 而 eframe 在定时重绘隐藏窗口时不带事件循环上下文，创建会失败并 panic。
    /// 延迟视口的窗口由 eframe 在下一轮事件里创建，搜索窗口也能独立重绘。
    ///
    /// 停止登记后，egui 会在下一次界面结束时移除这个子视口，eframe 随之关闭原生窗口。
    fn show_search_viewport(&mut self) {
        let Some(handle) = self.search.as_ref().cloned() else {
            self.close_search();
            return;
        };
        let id = search_window::viewport();
        let mode = lock_search(&handle).window_policy().mode;
        let builder = search_window::builder(mode, self.search_window_size, self.search_window_pos);
        // 窗口刚创建时系统激活可能还没完成：主动聚焦一次，之后不再抢焦点。
        if std::mem::take(&mut self.search_focus_pending) {
            self.ctx
                .send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        let close_requested = Arc::clone(&self.search_close_requested);
        let focus_was = Arc::clone(&self.search_focus_was);
        let blur_grace = Arc::clone(&self.search_blur_grace_until);
        let ui_handle = Arc::clone(&handle);
        self.ctx.show_viewport_deferred(id, builder, move |ui, _class| {
            // 搜索页画不透明底（§5.6）：普通窗口后面是桌面，玻璃/半透明会压低表格可读性。
            let background =
                Palette::with_alpha(ui.ctx().theme() == egui::Theme::Dark, 255).background;
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(background))
                .show(ui, |ui| {
                    let mut search = lock_search(&ui_handle);
                    let output = search.ui(ui);
                    let focused = ui.ctx().input(|i| i.viewport().focused).unwrap_or(false);
                    // 标题栏的关闭按钮：在子视口自己的输入里读取，避免事件被子视口重绘清掉。
                    let native_close = ui.ctx().input(|i| i.viewport().close_requested());
                    let lost_focus = focus_was.swap(focused, Ordering::Relaxed) && !focused;
                    if focused {
                        *blur_grace.lock().unwrap_or_else(|p| p.into_inner()) = None;
                    }
                    let hide_on_blur = search.window_policy().mode.hide_on_blur();
                    drop(search);
                    // 切换窗口模式会临时改变应用的激活策略，可能带一次瞬时失焦：只忽略宽限期内的失焦。
                    let transient_blur = lost_focus && {
                        let mut grace = blur_grace.lock().unwrap_or_else(|p| p.into_inner());
                        let within = grace.is_some_and(|until| Instant::now() < until);
                        *grace = None;
                        within
                    };
                    // Esc / 关闭按钮 / 浮动模式失焦：交给宿主在下一帧统一收起。
                    if native_close
                        || output.close
                        || (lost_focus && hide_on_blur && !transient_blur && !floating::dialog_open())
                    {
                        close_requested.store(true, Ordering::Relaxed);
                        // 延迟视口不会唤醒父窗口：主动让宿主跑一帧来处理关闭。
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                });
        });
        // 用户拖动边框缩放后，把内容区尺寸记下来（关闭窗口时统一走 RememberSize）。
        if let Some(rect) = self
            .ctx
            .input(|i| i.raw.viewports.get(&id).and_then(|v| v.inner_rect))
        {
            if rect.width() > 100.0 && rect.height() > 100.0 {
                lock_search(&handle)
                    .remember_size([rect.width().round(), rect.height().round()]);
            }
        }
    }

    /// 普通窗口需要出现在 Dock（macOS）/ 任务栏（Windows）；浮动、固定模式保持常驻托盘。
    fn apply_search_dock_visibility(&self) {
        let visible = self.search_open
            && self.search.as_ref().is_some_and(|handle| {
                lock_search(handle).window_policy().mode.in_taskbar()
            });
        native_panel::set_search_dock_visible(visible);
    }

    /// 按开关创建 / 释放搜索模块（关闭时不再占线程，§4.4）。
    fn sync_search_module(&mut self) {
        if self.settings.search_enabled {
            if self.search.is_none() && !self.search_stopping {
                let mut module = SearchModule::new(&self.data_dir, self.search_host.clone());
                module.warm_up();
                self.search = Some(Arc::new(Mutex::new(module)));
            }
        } else if let Some(handle) = &self.search {
            lock_search(handle).shutdown();
            self.search_stopping = true;
        }
        self.apply_hotkey(crate::hotkey::HotkeyAction::Search);
    }

    /// 拉取搜索模块的事件、响应策略变化、回收已停止的模块。
    fn poll_search(&mut self) {
        // 视口回调按 Esc / 关闭按钮 / 浮动失焦时置位。
        if self.search_close_requested.swap(false, Ordering::Relaxed) {
            self.close_search();
            return;
        }
        // 原生标题栏的关闭按钮（普通窗口）。
        if self.search_open
            && self.ctx.input(|i| {
                i.raw
                    .viewports
                    .get(&search_window::viewport())
                    .is_some_and(|v| v.close_requested())
            })
        {
            self.close_search();
            return;
        }
        let mut policy_changed = false;
        let mut stopped = false;
        if let Some(handle) = &self.search {
            let mut search = lock_search(handle);
            policy_changed = search.poll().policy_changed;
            stopped = search.is_stopped();
        }
        if policy_changed {
            // 普通 / 浮动 / 固定之间切换时同步 Dock 显示；窗口边框与层级由下一帧的 builder 更新。
            // 切换激活策略可能带一次瞬时失焦，浮动模式不要据此把窗口收起来。
            *self
                .search_blur_grace_until
                .lock()
                .unwrap_or_else(|p| p.into_inner()) =
                Some(Instant::now() + Duration::from_millis(600));
            self.apply_search_dock_visibility();
            if self.search_open {
                // 切换模式时窗口会重建 / 改层级，重新聚焦一次，避免切到浮动模式后停在“未聚焦”状态。
                self.ctx
                    .send_viewport_cmd_to(search_window::viewport(), egui::ViewportCommand::Focus);
            }
        }
        if self.search_stopping && stopped {
            self.search = None;
            self.search_stopping = false;
            if self.search_open {
                self.close_search();
            }
        }
        // 服务线程通过 ROOT 唤醒界面；延迟视口不会跟着父窗口重绘，这里转发一次。
        if self.search_open {
            self.ctx.request_repaint_of(search_window::viewport());
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
                if let Some(handle) = &self.search {
                    lock_search(handle).remember_size(size);
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
            Page::Search => self.search.as_ref().is_some_and(|handle| {
                lock_search(handle).window_policy().hide_on_blur
            }),
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

    /// 退出前：保存搜索窗口尺寸并停止索引（§4.6：托盘退出、更新重启都走这里）。
    fn on_exit(&mut self) {
        crate::shared::quit_trace("3 eframe: on_exit start");
        if self.search_open {
            crate::shared::quit_trace("4a on_exit: search_open branch, before lock_search");
            let size = self
                .ctx
                .input(|i| {
                    i.raw
                        .viewports
                        .get(&search_window::viewport())
                        .and_then(|v| v.inner_rect)
                })
                .map(|rect| [rect.width().round(), rect.height().round()]);
            if let Some(handle) = &self.search {
                let mut search = lock_search(handle);
                if let Some(size) = size {
                    if size[0] > 100.0 && size[1] > 100.0 {
                        search.remember_size(size);
                    }
                }
                search.on_hidden();
            }
            self.search_open = false;
        }
        crate::shared::quit_trace("4b on_exit: search_open branch done");
        if let Some(handle) = self.search.take() {
            // 有界阻塞：取消后等待线程池收尾，最多 1.5 秒。
            crate::shared::quit_trace("5 on_exit: shutdown_blocking search");
            let stopped = lock_search(&handle).shutdown_blocking(Duration::from_millis(1500));
            if !stopped {
                tracing::warn!("搜索模块未在 1.5 秒内停止，继续退出");
            }
        }
        crate::shared::quit_trace("6 on_exit: done");
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
        let search_needs_focus_poll = cfg!(windows)
            && self.search_open
            && self.search.as_ref().is_some_and(|handle| {
                lock_search(handle).window_policy().hide_on_blur
            });
        if (cfg!(windows) && self.page != Page::Settings && native_panel::is_visible())
            || search_needs_focus_poll
        {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        // 截屏遮罩或贴图在显示时，面板隐藏着 eframe 也会运行界面，这时不用绘制面板。
        if native_panel::is_visible() {
            self.draw(ui);
        }
        // 搜索页是独立窗口，每帧都要在这里登记延迟视口；停止登记后窗口随之关闭。
        if self.search_open {
            self.show_search_viewport();
        }
        let ctx = ui.ctx().clone();
        self.closing_snip_ui(&ctx);
        self.snip_ui(&ctx);
        self.pins_ui(&ctx);
        // 为创建截屏遮罩、贴图或搜索窗口而临时摆出的面板（见 `panel::wake`）完成了任务，收起来。
        // 隐藏着创建的遮罩还没显示时除外：面板一收起，eframe 就只调用 `logic`，没有人去显示遮罩了。
        // 搜索窗口打开期间保持唤醒：子视口被最小化后也要能收到父窗口的重绘与命令。
        if !self.search_open
            && !self
                .snip
                .as_ref()
                .is_some_and(|snip| snip.waiting_to_show())
        {
            native_panel::finish_wake();
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        // 窗口透明（macOS 毛玻璃 / 液态玻璃），圆角背景在 `draw` 里绘制；搜索页也一样，
        // 让系统材质透出来。
        if self.backdrop != Backdrop::Opaque {
            [0.0; 4]
        } else {
            Palette::new(visuals.dark_mode)
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
        // 搜索页与面板一样使用窗口材质：底色按“背景透明度”设置带 alpha，系统玻璃透出来。
        let background = palette.background;
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
        let Some(handle) = &self.search else {
            return;
        };
        let output = lock_search(handle).ui(ui);
        if output.close {
            self.pending_hide = true;
        }
    }
}
