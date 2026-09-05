use super::*;

/// 将剪贴板服务事件转发到核心运行时。
pub(super) fn start_clipboard_bridge(
    runtime: &Runtime,
    request_tx: mpsc::UnboundedSender<AppRequest>,
) {
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
pub(super) fn start_shortcut_bridge(
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
pub(super) fn start_tray_bridge(
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
pub(super) fn start_middle_bridge(
    runtime: &Runtime,
    app_weak: slint::Weak<SearchWindow>,
    storage: Storage,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    commands: Arc<Vec<CommandDescriptor>>,
    tool_usage: Arc<Mutex<HashMap<String, ToolUsage>>>,
    enabled: bool,
) -> MiddleClickController {
    let (quick_tx, mut quick_rx) = mpsc::unbounded_channel::<QuickActionEvent>();
    let controller = spawn_middle_click_listener(quick_tx, enabled);

    runtime.spawn(async move {
        while let Some(event) = quick_rx.recv().await {
            let app_weak = app_weak.clone();
            let storage = storage.clone();
            let request_tx = request_tx.clone();
            let commands = Arc::clone(&commands);
            let tool_usage = Arc::clone(&tool_usage);
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
                        commands,
                        tool_usage,
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
