//! Application composition root and shared application state.

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

// Slint window handles require strong references to remain open.
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

mod bridges;
mod callbacks;
mod clipboard_window;
mod event_handler;
mod item_actions;
mod presentation;
mod quick_actions;
mod timestamp_window;
mod tool_window;

use bridges::*;
use callbacks::*;
use clipboard_window::*;
use event_handler::*;
use item_actions::*;
use presentation::*;
use quick_actions::*;
use timestamp_window::*;
use tool_window::*;

pub fn run() -> Result<()> {
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
        Arc::clone(&commands),
        Arc::clone(&tool_usage),
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
    precreate_quick_action_window();
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
