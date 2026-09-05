use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::Result;
use devtools_core::{AppEvent, AppRequest, AppRuntime, SearchResult, TimestampConversionMode};
use devtools_plugin_api::{CommandDescriptor, CommandSource, PluginPermissions};
use devtools_plugin_host::{PluginRegistry, WasmPluginHost};
use devtools_services::{
    autostart::set_enabled as set_autostart_enabled,
    clipboard::{run_clipboard_watcher, set_clipboard_text, ClipboardEvent},
    middle_click::{
        spawn_middle_click_listener, MiddleClickController, QuickActionEvent, ScreenPosition,
    },
    shortcut::{
        spawn_global_shortcut_listener, update_global_shortcut, GlobalShortcut, ShortcutEvent,
    },
    tray::{spawn_tray_listener, TrayEvent, TrayPosition},
};
use devtools_storage::{ClipboardRecord, Storage, ToolHistoryRecord};
use devtools_ui::{
    ClipboardWindow, QuickActionWindow, SearchResultView, SearchWindow, TimestampWindow,
    ToolHistoryView, ToolWindow,
};
use slint::{
    CloseRequestResponse, ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel,
};
use time::{macros::format_description, OffsetDateTime, UtcOffset};
use tokio::{runtime::Runtime, sync::mpsc};
use tracing::{error, info, warn};

// Slint 窗口句柄需要保留强引用，否则窗口会被释放关闭。
thread_local! {
    static TOOL_WINDOWS: RefCell<Vec<ToolWindowState>> = const { RefCell::new(Vec::new()) };
    static TIMESTAMP_WINDOWS: RefCell<Vec<TimestampWindowState>> = const { RefCell::new(Vec::new()) };
    static CLIPBOARD_WINDOWS: RefCell<Vec<ClipboardWindowState>> = const { RefCell::new(Vec::new()) };
    static QUICK_WINDOWS: RefCell<Vec<QuickActionWindow>> = const { RefCell::new(Vec::new()) };
}

static SINGLE_TOOL_WINDOW: AtomicBool = AtomicBool::new(true);

struct ClipboardWindowState {
    window: ClipboardWindow,
    ids: Rc<RefCell<Vec<String>>>,
}

struct ToolWindowState {
    window: ToolWindow,
    history: Rc<RefCell<Vec<ToolHistoryRecord>>>,
}

struct TimestampWindowState {
    window: TimestampWindow,
    _timer: Rc<Timer>,
}

#[derive(Default)]
struct ToolUsage {
    last_used: u64,
    total: u32,
}

fn main() -> Result<()> {
    // 初始化日志，方便排查全局快捷键、托盘、剪贴板等平台能力。
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 存储和核心运行时先启动，UI 通过 channel 与其通信。
    let storage = Storage::open_default()?;
    let registry = PluginRegistry::scan([
        std::env::current_dir()?.join("plugins"),
        devtools_storage::data_dir()?.join("plugins"),
    ]);
    for diagnostic in registry.diagnostics() {
        warn!(%diagnostic, "plugin scan skipped an entry");
    }
    let plugin_host = Arc::new(WasmPluginHost::new(registry, PluginPermissions::default())?);
    let mut commands = devtools_tools::builtin_commands();
    commands.extend(plugin_host.registry().commands());
    let commands = Arc::new(commands);
    let hotkey = storage
        .get_setting("hotkey")?
        .unwrap_or_else(|| "Alt+Space".into());
    let autostart = storage
        .get_setting("autostart")?
        .map(|value| value == "true" || value == "1")
        .unwrap_or(false);
    let middle_click_enabled = storage
        .get_setting("middle_click_enabled")?
        .map(|value| value == "true" || value == "1")
        .unwrap_or(true);
    let single_tool_window = storage
        .get_setting("single_tool_window")?
        .map(|value| value == "true" || value == "1")
        .unwrap_or(true);
    SINGLE_TOOL_WINDOW.store(single_tool_window, Ordering::Relaxed);
    if autostart {
        if let Err(error) = set_autostart_enabled(true) {
            warn!(?error, "failed to restore autostart configuration");
        }
    }
    let runtime = Runtime::new()?;
    let (request_tx, request_rx) = mpsc::unbounded_channel::<AppRequest>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AppEvent>();

    runtime.spawn(
        AppRuntime::new(
            storage.clone(),
            commands.as_ref().clone(),
            plugin_host,
            request_tx.clone(),
            request_rx,
            event_tx,
        )
        .run(),
    );

    // 平台服务各自运行在后台，通过统一 AppRequest 进入核心运行时。
    start_clipboard_bridge(&runtime, request_tx.clone());
    let shortcut = start_shortcut_bridge(&runtime, request_tx.clone(), hotkey.clone());

    // 主窗口默认使用中文和深色主题，真正持久化设置会在 LoadTheme/SettingsLoaded 中覆盖。
    let app = SearchWindow::new()?;
    let app_weak = app.as_weak();
    let search_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let tool_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let tool_usage = Arc::new(Mutex::new(HashMap::<String, ToolUsage>::new()));

    app.set_active_view(0);
    app.set_dark_mode(true);
    app.set_language("zh-CN".into());
    app.set_settings_theme("dark".into());
    app.set_settings_hotkey(hotkey.clone().into());
    app.set_settings_autostart(autostart);
    app.set_settings_middle_click_enabled(middle_click_enabled);
    app.set_settings_single_tool_window(single_tool_window);
    app.set_settings_tool_history_limit("20".into());
    #[cfg(target_os = "macos")]
    {
        app.set_control_modifier("Ctrl".into());
        app.set_meta_modifier("Cmd".into());
    }
    #[cfg(not(target_os = "macos"))]
    {
        app.set_control_modifier("Ctrl".into());
        app.set_meta_modifier("Super".into());
    }
    app.set_tools(command_model(commands.as_ref().clone(), "zh-CN"));
    set_ids(
        &tool_ids,
        commands
            .iter()
            .filter(|command| command.id != "setting.theme")
            .cloned()
            .map(|cmd| cmd.id),
    );

    let middle_click = start_middle_bridge(
        &runtime,
        app_weak.clone(),
        storage.clone(),
        request_tx.clone(),
        middle_click_enabled,
    );
    bind_ui_callbacks(
        &app,
        request_tx.clone(),
        storage.clone(),
        Arc::clone(&search_ids),
        Arc::clone(&tool_ids),
        Arc::clone(&commands),
        Arc::clone(&tool_usage),
        shortcut,
        hotkey,
        middle_click,
    );
    bind_runtime_events(
        app_weak.clone(),
        Arc::clone(&search_ids),
        Arc::clone(&tool_ids),
        Arc::clone(&commands),
        Arc::clone(&tool_usage),
        request_tx.clone(),
        event_rx,
    );
    start_tray_bridge(&runtime, request_tx.clone(), app.as_weak(), storage.clone());
    // 预创建托盘剪贴板弹窗，首次左键点击时直接复用，避免卡顿和位置跳动。
    precreate_clipboard_popup(&storage);
    let close_app = app.as_weak();
    app.window().on_close_requested(move || {
        if let Some(app) = close_app.upgrade() {
            let _ = app.hide();
        }
        CloseRequestResponse::KeepWindowShown
    });

    request_tx.send(AppRequest::Search {
        query: String::new(),
    })?;
    request_tx.send(AppRequest::LoadTheme)?;
    app.show()?;
    slint::run_event_loop_until_quit()?;
    Ok(())
}

/// 将剪贴板服务事件转发到核心运行时。
fn start_clipboard_bridge(runtime: &Runtime, request_tx: mpsc::UnboundedSender<AppRequest>) {
    let (clipboard_tx, mut clipboard_rx) = mpsc::unbounded_channel::<ClipboardEvent>();
    runtime.spawn(run_clipboard_watcher(clipboard_tx));
    runtime.spawn(async move {
        while let Some(event) = clipboard_rx.recv().await {
            match event {
                ClipboardEvent::TextChanged {
                    content,
                    source_app,
                } => {
                    let _ = request_tx.send(AppRequest::ClipboardChanged {
                        content,
                        source_app,
                    });
                }
            }
        }
    });
}

/// 注册并桥接全局快捷键事件。
fn start_shortcut_bridge(
    runtime: &Runtime,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    hotkey: String,
) -> Option<GlobalShortcut> {
    let (shortcut_tx, mut shortcut_rx) = mpsc::unbounded_channel::<ShortcutEvent>();

    match spawn_global_shortcut_listener(shortcut_tx, Some(&hotkey)) {
        Ok(shortcut) => {
            info!(%hotkey, "global shortcut registered");
            runtime.spawn(async move {
                while let Some(event) = shortcut_rx.recv().await {
                    match event {
                        ShortcutEvent::ToggleWindow => {
                            let _ = request_tx.send(AppRequest::ToggleWindow);
                        }
                    }
                }
            });
            Some(shortcut)
        }
        Err(error) => {
            warn!(?error, "global shortcut unavailable; window remains usable");
            None
        }
    }
}

/// 注册并桥接系统托盘事件。
fn start_tray_bridge(
    runtime: &Runtime,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    app_weak: slint::Weak<SearchWindow>,
    storage: Storage,
) {
    let (tray_tx, mut tray_rx) = mpsc::unbounded_channel::<TrayEvent>();

    match spawn_tray_listener(tray_tx) {
        Ok(()) => info!("tray menu registered"),
        Err(error) => warn!(?error, "tray unavailable; app can still be closed normally"),
    }

    runtime.spawn(async move {
        while let Some(event) = tray_rx.recv().await {
            match event {
                TrayEvent::ShowWindow => {
                    let _ = request_tx.send(AppRequest::ToggleWindow);
                }
                TrayEvent::ShowClipboard { position } => {
                    let app_weak = app_weak.clone();
                    let storage = storage.clone();
                    if let Err(error) = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            open_clipboard_popup(
                                storage,
                                app.get_dark_mode(),
                                app.get_language().to_string(),
                                position,
                            );
                        }
                    }) {
                        error!(?error, "failed to open clipboard window from tray");
                    }
                }
                TrayEvent::Exit => {
                    let _ = request_tx.send(AppRequest::Exit);
                }
            }
        }
    });
}

/// 启动鼠标中键快捷动作桥接，事件最终回到 Slint UI 线程创建小窗口。
fn start_middle_bridge(
    runtime: &Runtime,
    app_weak: slint::Weak<SearchWindow>,
    storage: Storage,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    enabled: bool,
) -> MiddleClickController {
    let (quick_tx, mut quick_rx) = mpsc::unbounded_channel::<QuickActionEvent>();
    let controller = spawn_middle_click_listener(quick_tx, enabled);

    runtime.spawn(async move {
        while let Some(event) = quick_rx.recv().await {
            let app_weak = app_weak.clone();
            let storage = storage.clone();
            let request_tx = request_tx.clone();
            let result = slint::invoke_from_event_loop(move || match event {
                QuickActionEvent::Show {
                    selected_text,
                    position,
                } => {
                    let dark_mode = app_weak
                        .upgrade()
                        .map(|app| app.get_dark_mode())
                        .unwrap_or(true);
                    let language = app_weak
                        .upgrade()
                        .map(|app| app.get_language().to_string())
                        .unwrap_or_else(|| "zh-CN".into());
                    hide_clipboard_popups();
                    open_quick_action_window(
                        storage,
                        selected_text,
                        position,
                        dark_mode,
                        language,
                        request_tx,
                    );
                }
                QuickActionEvent::Dismiss => hide_transient_windows(),
            });
            if let Err(error) = result {
                error!(?error, "failed to open quick action window");
            }
        }
    });

    controller
}

/// 绑定 Slint 回调。UI 只发送请求或打开窗口，耗时逻辑放到运行时/服务层。
#[allow(clippy::too_many_arguments)]
fn bind_ui_callbacks(
    app: &SearchWindow,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    storage: Storage,
    search_ids: Arc<Mutex<Vec<String>>>,
    tool_ids: Arc<Mutex<Vec<String>>>,
    commands: Arc<Vec<CommandDescriptor>>,
    tool_usage: Arc<Mutex<HashMap<String, ToolUsage>>>,
    shortcut: Option<GlobalShortcut>,
    hotkey: String,
    middle_click: MiddleClickController,
) {
    let search_tx = request_tx.clone();
    // 搜索输入实时进入核心运行时，结果通过 AppEvent 异步回填。
    app.on_query_changed(move |query| {
        let _ = search_tx.send(AppRequest::Search {
            query: query.to_string(),
        });
    });

    let activate_app = app.as_weak();
    let activate_storage = storage.clone();
    let activate_tx = request_tx.clone();
    // 主搜索结果和工具列表共用激活逻辑，靠 ID 判断实际动作。
    app.on_activate_result(move |index| {
        if let Some(result_id) = get_id(&search_ids, index) {
            activate_item(
                &activate_app,
                activate_storage.clone(),
                activate_tx.clone(),
                &result_id,
                None,
            );
        }
    });

    let tools_app = app.as_weak();
    let tools_storage = storage.clone();
    let tools_tx = request_tx.clone();
    let activate_tool_ids = Arc::clone(&tool_ids);
    app.on_activate_tool(move |index| {
        if let Some(result_id) = get_id(&activate_tool_ids, index) {
            activate_item(
                &tools_app,
                tools_storage.clone(),
                tools_tx.clone(),
                &result_id,
                None,
            );
        }
    });

    let show_search_app = app.as_weak();
    let show_search_tx = request_tx.clone();
    app.on_show_search(move || {
        if let Some(app) = show_search_app.upgrade() {
            app.set_active_view(0);
            let _ = show_search_tx.send(AppRequest::Search {
                query: app.get_query().to_string(),
            });
        }
    });

    let show_tools_app = app.as_weak();
    let show_tools_commands = Arc::clone(&commands);
    let show_tools_usage = Arc::clone(&tool_usage);
    // tools 页面只展示工具类命令，并按当前语言生成列表文案。
    app.on_show_tools(move || {
        if let Some(app) = show_tools_app.upgrade() {
            let language = app.get_language().to_string();
            let mut commands = show_tools_commands
                .iter()
                .filter(|command| command.id != "setting.theme")
                .cloned()
                .collect::<Vec<_>>();
            sort_tool_commands(&mut commands, &show_tools_usage);
            set_ids(&tool_ids, commands.iter().map(|cmd| cmd.id.clone()));
            app.set_tools(command_model(commands, &language));
            app.set_active_view(1);
            app.set_status("".into());
        }
    });

    let settings_tx = request_tx.clone();
    app.on_show_settings(move || {
        let _ = settings_tx.send(AppRequest::OpenSettings);
    });

    let theme_tx = request_tx.clone();
    app.on_theme_selected(move |theme| {
        let _ = theme_tx.send(AppRequest::SetTheme {
            theme: theme.to_string(),
        });
    });

    let language_tx = request_tx.clone();
    let language_app = app.as_weak();
    let language_commands = Arc::clone(&commands);
    let language_usage = Arc::clone(&tool_usage);
    // 语言切换先立即更新 UI，再写入存储，减少界面反馈延迟。
    app.on_language_selected(move |language| {
        let language = language.to_string();
        if let Some(app) = language_app.upgrade() {
            app.set_language(language.clone().into());
            let mut commands = language_commands
                .iter()
                .filter(|command| command.id != "setting.theme")
                .cloned()
                .collect::<Vec<_>>();
            sort_tool_commands(&mut commands, &language_usage);
            app.set_tools(command_model(commands, &language));
            let _ = language_tx.send(AppRequest::Search {
                query: app.get_query().to_string(),
            });
        }
        let _ = language_tx.send(AppRequest::SetLanguage { language });
    });

    let hotkey_tx = request_tx.clone();
    let current_hotkey = Rc::new(RefCell::new(hotkey));
    let shortcut = Rc::new(RefCell::new(shortcut));
    let hotkey_app = app.as_weak();
    app.on_hotkey_changed(move |hotkey| {
        let hotkey = hotkey.to_string();
        let previous = current_hotkey.borrow().clone();
        let update_result = shortcut
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("global shortcut is unavailable"))
            .and_then(|shortcut| update_global_shortcut(shortcut, &hotkey));
        match update_result {
            Ok(()) => {
                *current_hotkey.borrow_mut() = hotkey.clone();
                let _ = hotkey_tx.send(AppRequest::SetHotkey { hotkey });
            }
            Err(error) => {
                if let Some(app) = hotkey_app.upgrade() {
                    app.set_settings_hotkey(previous.into());
                    app.set_status(format!("Shortcut unavailable: {error}").into());
                }
            }
        }
    });

    let autostart_tx = request_tx.clone();
    let autostart_app = app.as_weak();
    app.on_autostart_changed(move |enabled| match set_autostart_enabled(enabled) {
        Ok(()) => {
            let _ = autostart_tx.send(AppRequest::SetAutostart { enabled });
        }
        Err(error) => {
            if let Some(app) = autostart_app.upgrade() {
                app.set_settings_autostart(!enabled);
                app.set_status(format!("Autostart unavailable: {error}").into());
            }
        }
    });

    let middle_click_tx = request_tx.clone();
    let middle_click_app = app.as_weak();
    app.on_middle_click_enabled_changed(move |enabled| {
        middle_click.set_enabled(enabled);
        if let Err(error) = middle_click_tx.send(AppRequest::SetMiddleClickEnabled { enabled }) {
            middle_click.set_enabled(!enabled);
            if let Some(app) = middle_click_app.upgrade() {
                app.set_settings_middle_click_enabled(!enabled);
                app.set_status(format!("Middle-click unavailable: {error}").into());
            }
        }
    });

    let single_window_tx = request_tx.clone();
    app.on_single_tool_window_changed(move |enabled| {
        SINGLE_TOOL_WINDOW.store(enabled, Ordering::Relaxed);
        let _ = single_window_tx.send(AppRequest::SetSingleToolWindow { enabled });
    });

    let history_limit_tx = request_tx.clone();
    let history_limit_app = app.as_weak();
    app.on_tool_history_limit_changed(move |value| match value.parse::<usize>() {
        Ok(limit) if limit <= 1_000 => {
            let _ = history_limit_tx.send(AppRequest::SetToolHistoryLimit { limit });
        }
        _ => {
            if let Some(app) = history_limit_app.upgrade() {
                app.set_status(localized_status(
                    &app,
                    "历史条数需为 0 到 1000",
                    "History limit must be 0 to 1000",
                ));
            }
        }
    });

    app.on_clear_clipboard(move || {
        let _ = request_tx.send(AppRequest::ClearClipboard);
    });
}

/// 后台运行时事件必须切回 Slint 事件循环线程处理。
fn bind_runtime_events(
    app_weak: slint::Weak<SearchWindow>,
    search_ids: Arc<Mutex<Vec<String>>>,
    tool_ids: Arc<Mutex<Vec<String>>>,
    commands: Arc<Vec<CommandDescriptor>>,
    tool_usage: Arc<Mutex<HashMap<String, ToolUsage>>>,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    mut event_rx: mpsc::UnboundedReceiver<AppEvent>,
) {
    std::thread::Builder::new()
        .name("ui-event-bridge".into())
        .spawn(move || {
            let runtime = Runtime::new().expect("failed to create ui event bridge runtime");
            runtime.block_on(async move {
                while let Some(event) = event_rx.recv().await {
                    let app_weak = app_weak.clone();
                    let search_ids = Arc::clone(&search_ids);
                    let tool_ids = Arc::clone(&tool_ids);
                    let commands = Arc::clone(&commands);
                    let tool_usage = Arc::clone(&tool_usage);
                    let request_tx = request_tx.clone();
                    if let Err(error) = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            apply_event(
                                &app,
                                &search_ids,
                                &tool_ids,
                                &commands,
                                &tool_usage,
                                request_tx,
                                event,
                            );
                        }
                    }) {
                        error!(?error, "failed to dispatch UI event");
                        return;
                    }
                }
            });
        })
        .expect("failed to spawn ui event bridge");
}

/// 将核心事件应用到主窗口状态，所有 UI 属性更新集中在这里。
fn apply_event(
    app: &SearchWindow,
    search_ids: &Arc<Mutex<Vec<String>>>,
    tool_ids: &Arc<Mutex<Vec<String>>>,
    commands: &Arc<Vec<CommandDescriptor>>,
    tool_usage: &Arc<Mutex<HashMap<String, ToolUsage>>>,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    event: AppEvent,
) {
    match event {
        AppEvent::SearchCompleted(results) => {
            set_ids(search_ids, results.iter().map(|result| result.id.clone()));
            let language = app.get_language().to_string();
            app.set_results(results_to_model(results, &language));
        }
        AppEvent::ClipboardStored => {
            app.set_status(localized_status(app, "剪贴板已保存", "Clipboard saved"));
        }
        AppEvent::SettingsLoaded {
            theme,
            language,
            hotkey,
            autostart,
            middle_click_enabled,
            tool_history_limit,
            single_tool_window,
        } => {
            apply_theme(app, &theme);
            app.set_language(language.into());
            app.set_settings_hotkey(hotkey.into());
            app.set_settings_autostart(autostart);
            app.set_settings_middle_click_enabled(middle_click_enabled);
            app.set_settings_tool_history_limit(tool_history_limit.to_string().into());
            app.set_settings_single_tool_window(single_tool_window);
            SINGLE_TOOL_WINDOW.store(single_tool_window, Ordering::Relaxed);
            app.set_active_view(2);
            app.set_status("".into());
        }
        AppEvent::ThemeChanged { theme } => {
            apply_theme(app, &theme);
            app.set_status(localized_status(app, "主题已更新", "Theme updated"));
        }
        AppEvent::LanguageChanged { language } => {
            app.set_language(language.into());
            app.set_status(localized_status(app, "语言已更新", "Language updated"));
        }
        AppEvent::HotkeyChanged { hotkey } => {
            app.set_settings_hotkey(hotkey.into());
            app.set_status(localized_status(app, "快捷键已保存", "Hotkey saved"));
        }
        AppEvent::AutostartChanged { enabled } => {
            app.set_settings_autostart(enabled);
            app.set_status(localized_status(
                app,
                if enabled {
                    "已开启开机自启动"
                } else {
                    "已关闭开机自启动"
                },
                if enabled {
                    "Autostart enabled"
                } else {
                    "Autostart disabled"
                },
            ));
        }
        AppEvent::MiddleClickChanged { enabled } => {
            app.set_settings_middle_click_enabled(enabled);
            app.set_status(localized_status(
                app,
                if enabled {
                    "已开启鼠标中键快捷弹窗"
                } else {
                    "已关闭鼠标中键快捷弹窗"
                },
                if enabled {
                    "Middle-click actions enabled"
                } else {
                    "Middle-click actions disabled"
                },
            ));
        }
        AppEvent::CommandExecuted {
            command_id,
            title,
            content,
        } => {
            if let Some(command_id) = command_id {
                record_tool_usage(tool_usage, &command_id);
                refresh_tool_list(app, tool_ids, commands, tool_usage);
            }
            app.set_status(format!("{title}: {content}").into());
        }
        AppEvent::ToolCompleted {
            tool_id,
            title,
            content,
        } => {
            record_tool_usage(tool_usage, &tool_id);
            TOOL_WINDOWS.with(|windows| {
                for state in windows.borrow().iter() {
                    if state.window.get_tool_id() == tool_id {
                        state.window.set_output(content.clone().into());
                    }
                }
            });
            refresh_tool_list(app, tool_ids, commands, tool_usage);
            app.set_status(format!("{title}: {content}").into());
        }
        AppEvent::ToolHistoryUpdated { tool_id, history } => {
            TOOL_WINDOWS.with(|windows| {
                for state in windows.borrow().iter() {
                    if state.window.get_tool_id() == tool_id {
                        state.window.set_history(tool_history_model(&history));
                        *state.history.borrow_mut() = history.clone();
                    }
                }
            });
        }
        AppEvent::CopyRequested { text } => {
            if let Err(error) = set_clipboard_text(&text) {
                app.set_status(format!("Copy failed: {error}").into());
            }
        }
        AppEvent::ToolOpened {
            tool_id,
            title,
            history,
        } => {
            let language = app.get_language().to_string();
            if tool_id == "tool.timestamp.convert" {
                open_timestamp_window(app.get_dark_mode(), request_tx);
            } else {
                open_tool_window(
                    tool_id.clone(),
                    localized_title(&tool_id, &language, &title),
                    String::new(),
                    app.get_dark_mode(),
                    language,
                    request_tx,
                    history,
                );
            }
        }
        AppEvent::ToggleWindow => {
            app.window().show().ok();
            app.window().request_redraw();
        }
        AppEvent::ExitRequested => {
            slint::quit_event_loop().ok();
        }
        AppEvent::Error(message) => {
            app.set_status(format!("Error: {message}").into());
        }
        AppEvent::ToolHistoryLimitChanged { limit } => {
            app.set_settings_tool_history_limit(limit.to_string().into());
            app.set_status(localized_status(
                app,
                "工具历史设置已保存",
                "Tool history setting saved",
            ));
        }
        AppEvent::SingleToolWindowChanged { enabled } => {
            SINGLE_TOOL_WINDOW.store(enabled, Ordering::Relaxed);
            app.set_settings_single_tool_window(enabled);
            app.set_status(localized_status(
                app,
                if enabled {
                    "已启用工具单窗口"
                } else {
                    "已允许多个工具窗口"
                },
                if enabled {
                    "Single tool window enabled"
                } else {
                    "Multiple tool windows enabled"
                },
            ));
        }
        AppEvent::TimestampConverted { mode, value } => {
            TIMESTAMP_WINDOWS.with(|windows| {
                for state in windows.borrow().iter() {
                    match mode {
                        TimestampConversionMode::UnixToDatetime => {
                            state.window.set_datetime_output(value.clone().into());
                        }
                        TimestampConversionMode::DatetimeToUnix => {
                            state
                                .window
                                .set_datetime_timestamp_output(value.clone().into());
                        }
                        TimestampConversionMode::PartsToUnix => {
                            state.window.set_parts_output(value.clone().into());
                        }
                    }
                }
            });
        }
    }
}

/// 根据结果 ID 执行动作：设置和剪贴板由 UI 打开，其余命令统一交由 Core 执行。
fn activate_item(
    app_weak: &slint::Weak<SearchWindow>,
    storage: Storage,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    result_id: &str,
    _input: Option<String>,
) {
    let dark_mode = app_weak
        .upgrade()
        .map(|app| app.get_dark_mode())
        .unwrap_or(true);
    let language = app_weak
        .upgrade()
        .map(|app| app.get_language().to_string())
        .unwrap_or_else(|| "zh-CN".into());

    match result_id {
        "setting.theme" => {
            let _ = request_tx.send(AppRequest::OpenSettings);
        }
        "tool.clipboard.history" => {
            open_clipboard_window(storage, dark_mode, language);
        }
        _ => {
            let _ = request_tx.send(AppRequest::ActivateResult {
                result_id: result_id.to_string(),
            });
        }
    }
}

/// 打开独立工具窗口。工具窗口继承当前主题和语言，并支持置顶。
fn open_tool_window(
    tool_id: String,
    title: String,
    input: String,
    dark_mode: bool,
    language: String,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    history: Vec<ToolHistoryRecord>,
) {
    if SINGLE_TOOL_WINDOW.load(Ordering::Relaxed) {
        let has_existing = TOOL_WINDOWS.with(|windows| {
            let windows = windows.borrow();
            if let Some(state) = windows
                .iter()
                .find(|state| state.window.get_tool_id() == tool_id)
            {
                state.window.show().ok();
                state.window.window().request_redraw();
                true
            } else {
                false
            }
        });
        if has_existing {
            return;
        }
    }

    let window = ToolWindow::new().expect("failed to create tool window");
    window.set_tool_id(tool_id.clone().into());
    window.set_title_text(title.into());
    window.set_input(input.into());
    window.set_output("".into());
    window.set_dark_mode(dark_mode);
    window.set_language(language.into());
    window.set_pinned(false);
    window.set_history_visible(false);
    window.set_history(tool_history_model(&history));
    let history_records = Rc::new(RefCell::new(history));

    // UI 只发送意图，Core 决定执行内置工具或隔离的插件命令。
    let run_tool_id = tool_id.clone();
    let run_tx = request_tx.clone();
    window.on_run(move |input| {
        let _ = run_tx.send(AppRequest::RunTool {
            tool_id: run_tool_id.clone(),
            input: input.to_string(),
        });
    });

    let select_window = window.as_weak();
    let select_history = Rc::clone(&history_records);
    window.on_select_history(move |index| {
        let record = select_history.borrow().get(index as usize).cloned();
        if let (Some(window), Some(record)) = (select_window.upgrade(), record) {
            window.set_input(record.input.into());
            window.set_output(record.output.into());
        }
    });

    let history_window = window.as_weak();
    window.on_toggle_history(move || {
        if let Some(window) = history_window.upgrade() {
            window.set_history_visible(!window.get_history_visible());
        }
    });

    let copy_window = window.as_weak();
    window.on_copy_output(move || {
        if let Some(window) = copy_window.upgrade() {
            let _ = set_clipboard_text(&window.get_output());
        }
    });

    let pin_window = window.as_weak();
    window.on_toggle_pin(move || {
        if let Some(window) = pin_window.upgrade() {
            window.set_pinned(!window.get_pinned());
        }
    });

    window.show().ok();
    if !window.get_input().is_empty() {
        let _ = request_tx.send(AppRequest::RunTool {
            tool_id: tool_id.clone(),
            input: window.get_input().to_string(),
        });
    }
    TOOL_WINDOWS.with(|windows| {
        windows.borrow_mut().push(ToolWindowState {
            window,
            history: history_records,
        })
    });
}

/// 打开截图风格的时间戳工具窗口。默认单窗口模式下重复激活只聚焦现有窗口。
fn open_timestamp_window(dark_mode: bool, request_tx: mpsc::UnboundedSender<AppRequest>) {
    if SINGLE_TOOL_WINDOW.load(Ordering::Relaxed) {
        let has_existing = TIMESTAMP_WINDOWS.with(|windows| {
            let windows = windows.borrow();
            if let Some(state) = windows.first() {
                state.window.show().ok();
                state.window.window().request_redraw();
                true
            } else {
                false
            }
        });
        if has_existing {
            return;
        }
    }

    let window = TimestampWindow::new().expect("failed to create timestamp window");
    window.set_pinned(false);
    let now = devtools_tools::timestamp_now_with_unit(true);
    let datetime = devtools_tools::timestamp_now_datetime();
    let parts = datetime_parts(&datetime);
    window.set_dark_mode(dark_mode);
    window.set_current_unit_index(1);
    window.set_current_timestamp(now.clone().into());
    window.set_timestamp_input(now.into());
    window.set_datetime_output(datetime.clone().into());
    window.set_datetime_input(datetime.into());
    window.set_datetime_timestamp_output("".into());
    window.set_datetime_unit_index(1);
    window.set_year_input(parts[0].clone().into());
    window.set_month_input(parts[1].clone().into());
    window.set_day_input(parts[2].clone().into());
    window.set_hour_input(parts[3].clone().into());
    window.set_minute_input(parts[4].clone().into());
    window.set_second_input(parts[5].clone().into());
    window.set_parts_output("".into());
    window.set_parts_unit_index(1);

    let refresh_window = window.as_weak();
    window.on_refresh_current(move || {
        if let Some(window) = refresh_window.upgrade() {
            window.set_current_timestamp(
                devtools_tools::timestamp_now_with_unit(window.get_current_unit_index() == 1)
                    .into(),
            );
        }
    });

    let unit_window = window.as_weak();
    window.on_current_unit_changed(move |index| {
        if let Some(window) = unit_window.upgrade() {
            window
                .set_current_timestamp(devtools_tools::timestamp_now_with_unit(index == 1).into());
        }
    });

    let timer = Rc::new(Timer::default());
    let clock_window = window.as_weak();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        if let Some(window) = clock_window.upgrade() {
            window.set_current_timestamp(
                devtools_tools::timestamp_now_with_unit(window.get_current_unit_index() == 1)
                    .into(),
            );
        }
    });
    let start_timer = Rc::clone(&timer);
    window.on_start_clock(move || {
        start_timer.restart();
    });
    let stop_timer = Rc::clone(&timer);
    window.on_stop_clock(move || {
        stop_timer.stop();
    });

    let unix_tx = request_tx.clone();
    window.on_convert_timestamp(move |input| {
        let _ = unix_tx.send(AppRequest::TimestampConvertUnix {
            input: input.to_string(),
        });
    });
    let datetime_tx = request_tx.clone();
    window.on_convert_datetime(move |input, unit_index| {
        let _ = datetime_tx.send(AppRequest::TimestampConvertDatetime {
            input: input.to_string(),
            milliseconds: unit_index == 1,
        });
    });
    window.on_convert_parts(move |year, month, day, hour, minute, second, unit_index| {
        let _ = request_tx.send(AppRequest::TimestampConvertParts {
            year: year.to_string(),
            month: month.to_string(),
            day: day.to_string(),
            hour: hour.to_string(),
            minute: minute.to_string(),
            second: second.to_string(),
            milliseconds: unit_index == 1,
        });
    });

    let validate_window = window.as_weak();
    window.on_validate_parts_inputs(move || {
        let Some(window) = validate_window.upgrade() else {
            return;
        };
        let year = window.get_year_input().trim().to_string();
        let month = window.get_month_input().trim().to_string();
        let day = window.get_day_input().trim().to_string();
        let hour = window.get_hour_input().trim().to_string();
        let minute = window.get_minute_input().trim().to_string();
        let second = window.get_second_input().trim().to_string();
        let checks: [(&str, i64, i64, &str); 6] = [
            (year.as_str(), 0, 9999, "年份应为 0-9999 的数字"),
            (month.as_str(), 1, 12, "月份应为 1-12"),
            (day.as_str(), 1, 31, "日期应为 1-31"),
            (hour.as_str(), 0, 23, "小时应为 0-23"),
            (minute.as_str(), 0, 59, "分钟应为 0-59"),
            (second.as_str(), 0, 59, "秒应为 0-59"),
        ];
        let mut valid = [true; 6];
        let mut error = "";
        for (index, (text, min, max, message)) in checks.iter().enumerate() {
            if text.is_empty() {
                continue;
            }
            let ok = text.chars().all(|character: char| character.is_ascii_digit())
                && text
                    .parse::<i64>()
                    .map_or(false, |value| (*min..=*max).contains(&value));
            valid[index] = ok;
            if !ok && error.is_empty() {
                error = message;
            }
        }
        window.set_parts_year_valid(valid[0]);
        window.set_parts_month_valid(valid[1]);
        window.set_parts_day_valid(valid[2]);
        window.set_parts_hour_valid(valid[3]);
        window.set_parts_minute_valid(valid[4]);
        window.set_parts_second_valid(valid[5]);
        window.set_parts_error(error.into());
    });
    window.on_copy_text(move |text| {
        let _ = set_clipboard_text(&text);
    });

    let pin_window = window.as_weak();
    window.on_toggle_pin(move || {
        if let Some(window) = pin_window.upgrade() {
            window.set_pinned(!window.get_pinned());
        }
    });

    window.show().ok();
    TIMESTAMP_WINDOWS.with(|windows| {
        windows.borrow_mut().push(TimestampWindowState {
            window,
            _timer: timer,
        });
    });
}

fn datetime_parts(datetime: &str) -> [String; 6] {
    let values = datetime
        .split(|character: char| !character.is_ascii_digit())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    std::array::from_fn(|index| values.get(index).cloned().unwrap_or_default())
}

/// 打开剪贴板历史窗口，并维护 UI 行索引到数据库 ID 的映射。
fn open_clipboard_window(storage: Storage, dark_mode: bool, language: String) {
    open_clipboard_window_internal(storage, dark_mode, language, false, false, None);
}

/// 托盘左键打开的临时剪贴板弹窗：无标题栏、跟随托盘位置并自动置顶。
fn open_clipboard_popup(
    storage: Storage,
    dark_mode: bool,
    language: String,
    position: TrayPosition,
) {
    open_clipboard_window_internal(storage, dark_mode, language, true, false, Some(position));
}

/// 打开一个保留原生标题栏且已置顶的剪贴板窗口。
fn open_pinned_clipboard_window(storage: Storage, dark_mode: bool, language: String) {
    open_clipboard_window_internal(storage, dark_mode, language, false, true, None);
}

/// 启动时预创建托盘剪贴板弹窗（保持隐藏）。
/// 窗口创建是首次托盘点击卡顿的主要来源，提前创建后点击时只需刷新数据并显示。
/// 主题/语言在每次显示前都会按当前设置刷新，预创建时使用默认值即可。
fn precreate_clipboard_popup(storage: &Storage) {
    let state = build_clipboard_window(storage, true, "zh-CN", true, false);
    CLIPBOARD_WINDOWS.with(|windows| windows.borrow_mut().push(state));
}

fn open_clipboard_window_internal(
    storage: Storage,
    dark_mode: bool,
    language: String,
    popup_mode: bool,
    pinned: bool,
    position: Option<TrayPosition>,
) {
    if popup_mode {
        if show_existing_clipboard_popup(
            &storage,
            dark_mode,
            &language,
            position.expect("clipboard popup position is required"),
        ) {
            return;
        }
    } else if show_existing_clipboard_window(&storage, dark_mode, &language, pinned) {
        return;
    }

    let state = build_clipboard_window(&storage, dark_mode, &language, popup_mode, pinned);

    // 先定位再显示，避免窗口先出现在系统默认位置再跳动到托盘附近。
    if let Some(position) = position {
        set_clipboard_popup_position(&state.window, position);
    }
    state.window.show().ok();
    CLIPBOARD_WINDOWS.with(|windows| windows.borrow_mut().push(state));
}

/// 创建剪贴板窗口并绑定全部回调，但不负责显示。
fn build_clipboard_window(
    storage: &Storage,
    dark_mode: bool,
    language: &str,
    popup_mode: bool,
    pinned: bool,
) -> ClipboardWindowState {
    let window = ClipboardWindow::new().expect("failed to create clipboard window");
    let ids = Rc::new(RefCell::new(Vec::<String>::new()));
    window.set_popup_mode(popup_mode);
    window.set_pinned(pinned);
    window.set_dark_mode(dark_mode);
    window.set_language(language.into());
    // 隐藏状态下创建/显示的窗口不会触发 Slint show 时的 preferred 尺寸应用
    // (set_visible 走 Shown 路径被当作 recreating),会停在 min 尺寸 360x420,
    // 这里显式设为首选尺寸,保证窗口大小与托盘定位计算一致。
    window
        .window()
        .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(
            420.0, 620.0,
        )));
    refresh_clipboard_window(&window, storage, &ids, "", language);

    let query_window = window.as_weak();
    let query_storage = storage.clone();
    let query_ids = Rc::clone(&ids);
    let query_language = language.to_string();
    window.on_query_changed(move |query| {
        if let Some(window) = query_window.upgrade() {
            refresh_clipboard_window(&window, &query_storage, &query_ids, &query, &query_language);
        }
    });

    let activate_storage = storage.clone();
    let activate_ids = Rc::clone(&ids);
    let activate_window = window.as_weak();
    window.on_activate_item(move |index| {
        let id = activate_ids.borrow().get(index as usize).cloned();
        if let Some(raw_id) = id.and_then(|id| id.strip_prefix("clipboard:").map(str::to_string)) {
            if let Ok(id) = raw_id.parse::<i64>() {
                if let Ok(Some(record)) = activate_storage.clipboard_by_id(id) {
                    let _ = set_clipboard_text(&record.content);
                    if let Some(window) = activate_window.upgrade() {
                        if window.get_popup_mode() {
                            let _ = window.hide();
                        }
                    }
                }
            }
        }
    });

    let convert_window = window.as_weak();
    let convert_storage = storage.clone();
    let convert_language = language.to_string();
    let convert_dark_mode = dark_mode;
    window.on_convert_to_window(move || {
        if let Some(window) = convert_window.upgrade() {
            window.hide().ok();
            open_clipboard_window(
                convert_storage.clone(),
                convert_dark_mode,
                convert_language.clone(),
            );
        }
    });

    let pin_window = window.as_weak();
    let pin_storage = storage.clone();
    let pin_language = language.to_string();
    let pin_dark_mode = dark_mode;
    window.on_toggle_pin(move || {
        if let Some(window) = pin_window.upgrade() {
            if window.get_popup_mode() {
                window.hide().ok();
                open_pinned_clipboard_window(
                    pin_storage.clone(),
                    pin_dark_mode,
                    pin_language.clone(),
                );
            } else {
                window.set_pinned(!window.get_pinned());
            }
        }
    });

    ClipboardWindowState { window, ids }
}

/// 刷新并重新显示已有的剪贴板窗口，确保托盘重复点击不会创建窗口副本。
fn show_existing_clipboard_window(
    storage: &Storage,
    dark_mode: bool,
    language: &str,
    pin: bool,
) -> bool {
    CLIPBOARD_WINDOWS.with(|windows| {
        let windows = windows.borrow();
        let Some(state) = windows
            .iter()
            .rev()
            .find(|state| !state.window.get_popup_mode())
        else {
            return false;
        };

        if pin {
            state.window.set_pinned(true);
        }
        state.window.set_dark_mode(dark_mode);
        state.window.set_language(language.into());
        refresh_clipboard_window(&state.window, storage, &state.ids, "", language);
        // Re-showing after a hide asks the platform to activate and raise the window.
        state.window.hide().ok();
        state.window.show().ok();
        true
    })
}

/// 刷新并重新显示已有的托盘剪贴板弹窗，同时更新其位置。
fn show_existing_clipboard_popup(
    storage: &Storage,
    dark_mode: bool,
    language: &str,
    position: TrayPosition,
) -> bool {
    CLIPBOARD_WINDOWS.with(|windows| {
        let windows = windows.borrow();
        let Some(state) = windows
            .iter()
            .rev()
            .find(|state| state.window.get_popup_mode())
        else {
            return false;
        };

        state.window.set_dark_mode(dark_mode);
        state.window.set_language(language.into());
        refresh_clipboard_window(&state.window, storage, &state.ids, "", language);
        set_clipboard_popup_position(&state.window, position);
        state.window.hide().ok();
        state.window.show().ok();
        true
    })
}

fn set_clipboard_popup_position(window: &ClipboardWindow, position: TrayPosition) {
    // 窗口尺寸已在创建时显式设为 420x620 逻辑像素,这里按逻辑尺寸×scale 计算。
    // 注意不能用 window.size():隐藏窗口的尺寸缓存可能是旧值(见 build_clipboard_window)。
    let scale = window.window().scale_factor().max(1.0);
    let width = (420.0 * scale).round() as i32;
    #[cfg(not(target_os = "macos"))]
    let height = (620.0 * scale).round() as i32;

    #[cfg(target_os = "macos")]
    let x = position.x.saturating_sub(width / 2);
    #[cfg(not(target_os = "macos"))]
    let x = position.x.saturating_sub(width).saturating_add(32);

    #[cfg(target_os = "macos")]
    let y = position.y.saturating_add(8);
    #[cfg(not(target_os = "macos"))]
    let y = position.y.saturating_sub(height).saturating_sub(8);

    window
        .window()
        .set_position(slint::PhysicalPosition::new(x, y));
}

/// 打开中键快捷动作窗口，动作列表会根据选中文本内容做简单推荐。
fn open_quick_action_window(
    storage: Storage,
    selected_text: Option<String>,
    position: Option<ScreenPosition>,
    dark_mode: bool,
    language: String,
    request_tx: mpsc::UnboundedSender<AppRequest>,
) {
    let text = selected_text.unwrap_or_default();
    let actions = quick_actions_for(&text, &language);
    let action_ids = Rc::new(RefCell::new(
        actions
            .iter()
            .map(|action| action.id.clone())
            .collect::<Vec<_>>(),
    ));

    let window = QuickActionWindow::new().expect("failed to create quick action window");
    window.set_selected_text(text.clone().into());
    window.set_actions(results_to_model(actions, &language));
    window.set_dark_mode(dark_mode);
    window.set_language(language.clone().into());
    if let Some(position) = position {
        set_quick_action_position(&window, position);
    }

    let activate_window = window.as_weak();
    let activate_language = language.clone();
    let activate_tx = request_tx.clone();
    window.on_activate_action(move |index| {
        let action_id = action_ids.borrow().get(index as usize).cloned();
        if let Some(action_id) = action_id {
            if action_id == "tool.clipboard.history" {
                open_clipboard_window(storage.clone(), dark_mode, activate_language.clone());
            } else {
                open_tool_window(
                    action_id.clone(),
                    devtools_tools::localized_title_for(&action_id, &activate_language),
                    text.clone(),
                    dark_mode,
                    activate_language.clone(),
                    activate_tx.clone(),
                    Vec::new(),
                );
            }
            if let Some(window) = activate_window.upgrade() {
                window.hide().ok();
            }
        }
    });

    window.show().ok();
    QUICK_WINDOWS.with(|windows| windows.borrow_mut().push(window));
}

/// 将快捷窗口放到中键位置右下方。Quartz 使用逻辑坐标，Windows/rdev 使用物理坐标。
fn set_quick_action_position(window: &QuickActionWindow, position: ScreenPosition) {
    #[cfg(target_os = "macos")]
    window.window().set_position(slint::LogicalPosition::new(
        position.x as f32 + 12.0,
        position.y as f32 + 12.0,
    ));

    #[cfg(not(target_os = "macos"))]
    window.window().set_position(slint::PhysicalPosition::new(
        position.x.saturating_add(12),
        position.y.saturating_add(12),
    ));
}

/// 隐藏所有已经创建的快捷动作窗口，避免外部点击后仍有旧窗口留在最前面。
fn hide_quick_action_windows() {
    QUICK_WINDOWS.with(|windows| {
        for window in windows.borrow().iter() {
            if window.window().is_visible() {
                let _ = window.hide();
            }
        }
    });
}

/// 隐藏托盘打开的临时剪贴板弹窗，普通剪贴板窗口不受影响。
fn hide_clipboard_popups() {
    CLIPBOARD_WINDOWS.with(|windows| {
        for state in windows.borrow().iter() {
            if state.window.get_popup_mode() && state.window.window().is_visible() {
                let _ = state.window.hide();
            }
        }
    });
}

fn hide_transient_windows() {
    hide_quick_action_windows();
    hide_clipboard_popups();
}

/// 重新加载剪贴板窗口列表，并同步更新行 ID 映射。
fn refresh_clipboard_window(
    window: &ClipboardWindow,
    storage: &Storage,
    ids: &Rc<RefCell<Vec<String>>>,
    query: &str,
    language: &str,
) {
    let rows = storage.search_clipboard(query, 100).unwrap_or_default();
    window.set_item_count(clipboard_count(rows.len(), language).into());
    *ids.borrow_mut() = rows
        .iter()
        .map(|record| format!("clipboard:{}", record.id))
        .collect();
    window.set_rows(clipboard_model(rows, language));
}

/// 根据选中文本生成快捷动作。JSON 外形的文本优先推荐 JSON 工具。
fn quick_actions_for(selected_text: &str, language: &str) -> Vec<SearchResult> {
    let mut ids = Vec::new();
    if selected_text.trim_start().starts_with('{') || selected_text.trim_start().starts_with('[') {
        ids.push("tool.json.format");
        ids.push("tool.json.minify");
        ids.push("tool.json.validate");
    }
    ids.push("tool.base64.encode");
    ids.push("tool.base64.decode");
    ids.push("tool.clipboard.history");

    ids.into_iter()
        .map(|id| SearchResult {
            id: id.into(),
            title: devtools_tools::localized_title_for(id, language),
            subtitle: if selected_text.is_empty() {
                if language == "en" {
                    "Open tool".into()
                } else {
                    "打开工具".into()
                }
            } else {
                if language == "en" {
                    "Use selected text".into()
                } else {
                    "使用选中文本".into()
                }
            },
            source: devtools_core::search::SearchSource::BuiltInTool,
            score: 1.0,
        })
        .collect()
}

/// 应用主题到主窗口状态。
fn apply_theme(app: &SearchWindow, theme: &str) {
    app.set_settings_theme(theme.into());
    app.set_dark_mode(theme != "light");
}

/// 根据当前语言选择状态提示文本。
fn localized_status(app: &SearchWindow, zh: &str, en: &str) -> SharedString {
    if app.get_language() == "en" {
        en.into()
    } else {
        zh.into()
    }
}

/// 从行索引查找真实结果 ID。
fn get_id(ids: &Arc<Mutex<Vec<String>>>, index: i32) -> Option<String> {
    ids.lock()
        .ok()
        .and_then(|ids| ids.get(index as usize).cloned())
}

/// 更新行索引到结果 ID 的映射。
fn set_ids(ids: &Arc<Mutex<Vec<String>>>, values: impl Iterator<Item = String>) {
    if let Ok(mut ids) = ids.lock() {
        *ids = values.collect();
    }
}

/// 本次启动内用过的工具优先；最近一次使用优先，再按累计使用次数排序。
fn sort_tool_commands(
    commands: &mut [CommandDescriptor],
    usage: &Arc<Mutex<HashMap<String, ToolUsage>>>,
) {
    let Ok(usage) = usage.lock() else {
        return;
    };
    commands.sort_by(
        |left, right| match (usage.get(&left.id), usage.get(&right.id)) {
            (Some(left_usage), Some(right_usage)) => right_usage
                .last_used
                .cmp(&left_usage.last_used)
                .then_with(|| right_usage.total.cmp(&left_usage.total))
                .then_with(|| left.title.cmp(&right.title)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.title.cmp(&right.title),
        },
    );
}

fn record_tool_usage(usage: &Arc<Mutex<HashMap<String, ToolUsage>>>, tool_id: &str) {
    if let Ok(mut usage) = usage.lock() {
        let next_sequence = usage
            .values()
            .map(|entry| entry.last_used)
            .max()
            .unwrap_or(0)
            + 1;
        let entry = usage.entry(tool_id.to_string()).or_default();
        entry.last_used = next_sequence;
        entry.total += 1;
    }
}

fn refresh_tool_list(
    app: &SearchWindow,
    tool_ids: &Arc<Mutex<Vec<String>>>,
    commands: &Arc<Vec<CommandDescriptor>>,
    usage: &Arc<Mutex<HashMap<String, ToolUsage>>>,
) {
    let mut commands = commands
        .iter()
        .filter(|command| command.id != "setting.theme")
        .cloned()
        .collect::<Vec<_>>();
    sort_tool_commands(&mut commands, usage);
    set_ids(tool_ids, commands.iter().map(|command| command.id.clone()));
    let language = app.get_language().to_string();
    app.set_tools(command_model(commands, &language));
}

fn tool_history_model(records: &[ToolHistoryRecord]) -> ModelRc<ToolHistoryView> {
    let rows = records
        .iter()
        .map(|record| ToolHistoryView {
            summary: summarize(&record.input).into(),
            created_at: format_clipboard_time(record.created_at).into(),
        })
        .collect::<Vec<_>>();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// 将命令描述符转换成 Slint 列表模型。
fn command_model(commands: Vec<CommandDescriptor>, language: &str) -> ModelRc<SearchResultView> {
    let rows = commands
        .into_iter()
        .map(|command| {
            let (title, subtitle) = devtools_tools::localized_command_text(&command, language);
            SearchResultView {
                id: command.id.into(),
                title: title.into(),
                subtitle: subtitle.into(),
                source: source_label_from_command(&command.source, language),
                meta: "".into(),
                shortcut: "".into(),
            }
        })
        .collect::<Vec<_>>();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// 将剪贴板记录转换成 Slint 列表模型，并按语言显示来源标签。
fn clipboard_model(records: Vec<ClipboardRecord>, language: &str) -> ModelRc<SearchResultView> {
    let rows = records
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            let source_app = record.source_app.unwrap_or_else(|| {
                if language == "en" {
                    "Local clipboard".into()
                } else {
                    "本地剪贴板".into()
                }
            });
            let title = if record.sensitive {
                format!("{}...", record.content.chars().take(12).collect::<String>())
            } else {
                summarize(&record.content)
            };
            let source = source_app.chars().take(2).collect::<String>();
            SearchResultView {
                id: format!("clipboard:{}", record.id).into(),
                title: title.into(),
                subtitle: source_app.into(),
                source: source.into(),
                meta: format_clipboard_time(record.created_at).into(),
                shortcut: clipboard_shortcut(index).into(),
            }
        })
        .collect::<Vec<_>>();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

fn clipboard_count(count: usize, language: &str) -> String {
    if language == "en" {
        format!("{count} items")
    } else {
        format!("共 {count} 项")
    }
}

fn format_clipboard_time(timestamp: i64) -> String {
    let Ok(utc_time) = OffsetDateTime::from_unix_timestamp(timestamp) else {
        return String::new();
    };
    let east8 = UtcOffset::from_hms(8, 0, 0).expect("+08:00 must be valid");
    utc_time
        .to_offset(east8)
        .format(&format_description!("[hour]:[minute]"))
        .unwrap_or_default()
}

fn clipboard_shortcut(index: usize) -> String {
    if index >= 9 {
        return String::new();
    }
    let modifier = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl"
    };
    format!("{modifier} {}", index + 1)
}

/// 将核心搜索结果转换成 Slint 列表模型。
fn results_to_model(results: Vec<SearchResult>, language: &str) -> ModelRc<SearchResultView> {
    let rows = results
        .into_iter()
        .map(|result| {
            let title = localized_title(&result.id, language, &result.title);
            let subtitle = localized_subtitle(&result.id, language, &result.subtitle);
            SearchResultView {
                id: result.id.into(),
                title: title.into(),
                subtitle: subtitle.into(),
                source: source_label(&result.source, language),
                meta: "".into(),
                shortcut: "".into(),
            }
        })
        .collect::<Vec<_>>();

    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// 将命令来源本地化成短标签，显示在结果行左侧徽标中。
fn source_label_from_command(source: &CommandSource, language: &str) -> SharedString {
    match source {
        CommandSource::BuiltInTool => if language == "en" { "Tool" } else { "工具" }.into(),
        CommandSource::Plugin => if language == "en" { "Plugin" } else { "插件" }.into(),
        CommandSource::Clipboard => if language == "en" { "Clip" } else { "剪贴" }.into(),
        CommandSource::History => if language == "en" {
            "History"
        } else {
            "历史"
        }
        .into(),
        CommandSource::Setting => if language == "en" {
            "Setting"
        } else {
            "设置"
        }
        .into(),
    }
}

/// 将搜索来源本地化成短标签。
fn source_label(source: &devtools_core::search::SearchSource, language: &str) -> SharedString {
    match source {
        devtools_core::search::SearchSource::BuiltInTool => {
            if language == "en" { "Tool" } else { "工具" }.into()
        }
        devtools_core::search::SearchSource::Clipboard => {
            if language == "en" { "Clip" } else { "剪贴" }.into()
        }
        devtools_core::search::SearchSource::Plugin => {
            if language == "en" { "Plugin" } else { "插件" }.into()
        }
        devtools_core::search::SearchSource::History => if language == "en" {
            "History"
        } else {
            "历史"
        }
        .into(),
        devtools_core::search::SearchSource::Setting => if language == "en" {
            "Setting"
        } else {
            "设置"
        }
        .into(),
    }
}

/// 根据结果 ID 尝试读取本地化标题，找不到时使用搜索结果原文。
fn localized_title(result_id: &str, language: &str, fallback: &str) -> String {
    if result_id.starts_with("tool.") || result_id.starts_with("setting.") {
        devtools_tools::localized_title_for(result_id, language)
    } else {
        fallback.to_string()
    }
}

/// 根据结果 ID 尝试读取本地化副标题。
fn localized_subtitle(result_id: &str, language: &str, fallback: &str) -> String {
    if result_id.starts_with("tool.") || result_id.starts_with("setting.") {
        devtools_tools::localized_subtitle_for(result_id, language)
    } else {
        fallback.to_string()
    }
}

/// 压缩长文本摘要，避免剪贴板列表行被超长内容撑开。
fn summarize(content: &str) -> String {
    let single_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() > 72 {
        format!("{}...", single_line.chars().take(72).collect::<String>())
    } else {
        single_line
    }
}
