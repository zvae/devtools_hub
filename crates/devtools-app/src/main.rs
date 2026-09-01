use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use devtools_core::{AppEvent, AppRequest, AppRuntime, SearchResult};
use devtools_plugin_api::{CommandDescriptor, CommandSource};
use devtools_services::{
    clipboard::{run_clipboard_watcher, set_clipboard_text, ClipboardEvent},
    middle_click::{spawn_middle_click_listener, QuickActionEvent, ScreenPosition},
    shortcut::{spawn_global_shortcut_listener, ShortcutEvent},
    tray::{spawn_tray_listener, TrayEvent, TrayPosition},
};
use devtools_storage::{ClipboardRecord, Storage};
use devtools_ui::{ClipboardWindow, QuickActionWindow, SearchResultView, SearchWindow, ToolWindow};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
#[cfg(not(unix))]
use time::{macros::format_description, OffsetDateTime};
use tokio::{runtime::Runtime, sync::mpsc};
use tracing::{error, info, warn};

// Slint 窗口句柄需要保留强引用，否则窗口会被释放关闭。
thread_local! {
    static TOOL_WINDOWS: RefCell<Vec<ToolWindow>> = const { RefCell::new(Vec::new()) };
    static CLIPBOARD_WINDOWS: RefCell<Vec<ClipboardWindowState>> = const { RefCell::new(Vec::new()) };
    static QUICK_WINDOWS: RefCell<Vec<QuickActionWindow>> = const { RefCell::new(Vec::new()) };
}

struct ClipboardWindowState {
    window: ClipboardWindow,
    ids: Rc<RefCell<Vec<String>>>,
}

fn main() -> Result<()> {
    // 初始化日志，方便排查全局快捷键、托盘、剪贴板等平台能力。
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 存储和核心运行时先启动，UI 通过 channel 与其通信。
    let storage = Storage::open_default()?;
    let hotkey = storage
        .get_setting("hotkey")?
        .unwrap_or_else(|| "Alt+Space".into());
    let runtime = Runtime::new()?;
    let (request_tx, request_rx) = mpsc::unbounded_channel::<AppRequest>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AppEvent>();

    runtime.spawn(AppRuntime::new(storage.clone(), request_rx, event_tx).run());

    // 平台服务各自运行在后台，通过统一 AppRequest 进入核心运行时。
    start_clipboard_bridge(&runtime, request_tx.clone());
    start_shortcut_bridge(&runtime, request_tx.clone(), hotkey);

    // 主窗口默认使用中文和深色主题，真正持久化设置会在 LoadTheme/SettingsLoaded 中覆盖。
    let app = SearchWindow::new()?;
    let app_weak = app.as_weak();
    let search_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let tool_ids = Arc::new(Mutex::new(Vec::<String>::new()));

    app.set_active_view(0);
    app.set_dark_mode(true);
    app.set_language("zh-CN".into());
    app.set_settings_theme("dark".into());
    app.set_settings_hotkey("Alt+Space".into());
    app.set_tools(command_model(devtools_tools::builtin_commands(), "zh-CN"));
    set_ids(
        &tool_ids,
        devtools_tools::builtin_commands()
            .into_iter()
            .map(|cmd| cmd.id),
    );

    bind_ui_callbacks(
        &app,
        request_tx.clone(),
        storage.clone(),
        Arc::clone(&search_ids),
        Arc::clone(&tool_ids),
    );
    bind_runtime_events(app_weak.clone(), Arc::clone(&search_ids), event_rx);
    start_tray_bridge(&runtime, request_tx.clone(), app.as_weak(), storage.clone());
    start_middle_bridge(&runtime, app_weak, storage);

    request_tx.send(AppRequest::Search {
        query: String::new(),
    })?;
    request_tx.send(AppRequest::LoadTheme)?;
    app.run()?;
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
) {
    let (shortcut_tx, mut shortcut_rx) = mpsc::unbounded_channel::<ShortcutEvent>();

    match spawn_global_shortcut_listener(shortcut_tx, Some(&hotkey)) {
        Ok(()) => info!(%hotkey, "global shortcut registered"),
        Err(error) => warn!(?error, "global shortcut unavailable; window remains usable"),
    }

    runtime.spawn(async move {
        while let Some(event) = shortcut_rx.recv().await {
            match event {
                ShortcutEvent::ToggleWindow => {
                    let _ = request_tx.send(AppRequest::ToggleWindow);
                }
            }
        }
    });
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
fn start_middle_bridge(runtime: &Runtime, app_weak: slint::Weak<SearchWindow>, storage: Storage) {
    let (quick_tx, mut quick_rx) = mpsc::unbounded_channel::<QuickActionEvent>();
    spawn_middle_click_listener(quick_tx);

    runtime.spawn(async move {
        while let Some(event) = quick_rx.recv().await {
            let app_weak = app_weak.clone();
            let storage = storage.clone();
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
                    open_quick_action_window(storage, selected_text, position, dark_mode, language);
                }
                QuickActionEvent::Dismiss => hide_transient_windows(),
            });
            if let Err(error) = result {
                error!(?error, "failed to open quick action window");
            }
        }
    });
}

/// 绑定 Slint 回调。UI 只发送请求或打开窗口，耗时逻辑放到运行时/服务层。
fn bind_ui_callbacks(
    app: &SearchWindow,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    storage: Storage,
    search_ids: Arc<Mutex<Vec<String>>>,
    tool_ids: Arc<Mutex<Vec<String>>>,
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
    // tools 页面只展示工具类命令，并按当前语言生成列表文案。
    app.on_show_tools(move || {
        if let Some(app) = show_tools_app.upgrade() {
            let language = app.get_language().to_string();
            let commands = devtools_tools::builtin_commands()
                .into_iter()
                .filter(|command| command.id.starts_with("tool."))
                .collect::<Vec<_>>();
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
    // 语言切换先立即更新 UI，再写入存储，减少界面反馈延迟。
    app.on_language_selected(move |language| {
        let language = language.to_string();
        if let Some(app) = language_app.upgrade() {
            app.set_language(language.clone().into());
            let commands = devtools_tools::builtin_commands()
                .into_iter()
                .filter(|command| command.id.starts_with("tool."))
                .collect::<Vec<_>>();
            app.set_tools(command_model(commands, &language));
            let _ = language_tx.send(AppRequest::Search {
                query: app.get_query().to_string(),
            });
        }
        let _ = language_tx.send(AppRequest::SetLanguage { language });
    });

    let hotkey_tx = request_tx.clone();
    app.on_hotkey_changed(move |hotkey| {
        let _ = hotkey_tx.send(AppRequest::SetHotkey {
            hotkey: hotkey.to_string(),
        });
    });

    app.on_clear_clipboard(move || {
        let _ = request_tx.send(AppRequest::ClearClipboard);
    });
}

/// 后台运行时事件必须切回 Slint 事件循环线程处理。
fn bind_runtime_events(
    app_weak: slint::Weak<SearchWindow>,
    search_ids: Arc<Mutex<Vec<String>>>,
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
                    if let Err(error) = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_weak.upgrade() {
                            apply_event(&app, &search_ids, event);
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
fn apply_event(app: &SearchWindow, search_ids: &Arc<Mutex<Vec<String>>>, event: AppEvent) {
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
        } => {
            apply_theme(app, &theme);
            app.set_language(language.into());
            app.set_settings_hotkey(hotkey.into());
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
            app.set_status(localized_status(
                app,
                "快捷键已保存，重启后生效",
                "Hotkey saved. Restart to apply.",
            ));
        }
        AppEvent::CommandExecuted { title, content }
        | AppEvent::ToolCompleted { title, content } => {
            app.set_status(format!("{title}: {content}").into());
        }
        AppEvent::CopyRequested { text } => {
            if let Err(error) = set_clipboard_text(&text) {
                app.set_status(format!("Copy failed: {error}").into());
            }
        }
        AppEvent::ToolOpened { tool_id, title } => {
            let language = app.get_language().to_string();
            open_tool_window(
                tool_id.clone(),
                localized_title(&tool_id, &language, &title),
                String::new(),
                app.get_dark_mode(),
                language,
            );
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
    }
}

/// 根据结果 ID 执行动作：设置项打开设置，剪贴板打开历史窗口，其它工具打开工具窗口。
fn activate_item(
    app_weak: &slint::Weak<SearchWindow>,
    storage: Storage,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    result_id: &str,
    input: Option<String>,
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
        "tool.uuid.v4" => {
            let result = devtools_tools::execute(result_id, "");
            let _ = set_clipboard_text(&result.content);
            if let Some(app) = app_weak.upgrade() {
                app.set_status(localized_status(&app, "UUID 已复制", "UUID copied"));
            }
        }
        id if id.starts_with("tool.") => {
            open_tool_window(
                id.to_string(),
                devtools_tools::localized_title_for(id, &language),
                input.unwrap_or_default(),
                dark_mode,
                language,
            );
        }
        _ => {}
    }
}

/// 打开独立工具窗口。工具窗口继承当前主题和语言，并支持置顶。
fn open_tool_window(
    tool_id: String,
    title: String,
    input: String,
    dark_mode: bool,
    language: String,
) {
    let window = ToolWindow::new().expect("failed to create tool window");
    window.set_tool_id(tool_id.clone().into());
    window.set_title_text(title.into());
    window.set_input(input.into());
    window.set_output("".into());
    window.set_dark_mode(dark_mode);
    window.set_language(language.into());
    window.set_pinned(false);

    // 工具运行直接在 UI 回调中执行，当前内置工具都是轻量文本处理。
    let run_window = window.as_weak();
    let run_tool_id = tool_id.clone();
    window.on_run(move |input| {
        let result = devtools_tools::execute(&run_tool_id, &input);
        if let Some(window) = run_window.upgrade() {
            window.set_output(result.content.into());
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
    TOOL_WINDOWS.with(|windows| windows.borrow_mut().push(window));
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

    let window = ClipboardWindow::new().expect("failed to create clipboard window");
    let ids = Rc::new(RefCell::new(Vec::<String>::new()));
    window.set_popup_mode(popup_mode);
    window.set_pinned(pinned);
    window.set_dark_mode(dark_mode);
    window.set_language(language.clone().into());
    refresh_clipboard_window(&window, &storage, &ids, "", &language);

    let query_window = window.as_weak();
    let query_storage = storage.clone();
    let query_ids = Rc::clone(&ids);
    let query_language = language.clone();
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
    let convert_language = language.clone();
    window.on_convert_to_window(move || {
        if let Some(window) = convert_window.upgrade() {
            window.hide().ok();
            open_clipboard_window(convert_storage.clone(), dark_mode, convert_language.clone());
        }
    });

    let pin_window = window.as_weak();
    let pin_storage = storage.clone();
    let pin_language = language.clone();
    window.on_toggle_pin(move || {
        if let Some(window) = pin_window.upgrade() {
            if window.get_popup_mode() {
                window.hide().ok();
                open_pinned_clipboard_window(pin_storage.clone(), dark_mode, pin_language.clone());
            } else {
                window.set_pinned(!window.get_pinned());
            }
        }
    });

    window.show().ok();
    if let Some(position) = position {
        set_clipboard_popup_position(&window, position);
    }
    CLIPBOARD_WINDOWS.with(|windows| {
        windows
            .borrow_mut()
            .push(ClipboardWindowState { window, ids });
    });
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
                source: source_label_from_command(&command.source, language).into(),
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
    #[cfg(unix)]
    {
        let timestamp = timestamp as libc::time_t;
        let mut local_time = std::mem::MaybeUninit::<libc::tm>::uninit();
        let result = unsafe { libc::localtime_r(&timestamp, local_time.as_mut_ptr()) };
        if result.is_null() {
            return String::new();
        }
        let local_time = unsafe { local_time.assume_init() };
        return format!("{:02}:{:02}", local_time.tm_hour, local_time.tm_min);
    }

    #[cfg(not(unix))]
    {
        let Ok(utc_time) = OffsetDateTime::from_unix_timestamp(timestamp) else {
            return String::new();
        };
        utc_time
            .format(&format_description!("[hour]:[minute]"))
            .unwrap_or_default()
    }
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
                source: source_label(&result.source, language).into(),
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
