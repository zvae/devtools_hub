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

/// macOS 平台：监听应用失活与窗口失焦通知，点击浮窗外任意位置时立即隐藏剪贴板与快捷动作弹窗。
/// 覆盖两类场景：点击其他应用（应用失活）、点击本应用其他窗口或桌面（窗口失焦）。
/// NSNotificationCenter 回调运行在主线程，invoke_from_event_loop 切回 Slint UI 线程访问 thread_local 状态。
#[cfg(target_os = "macos")]
pub(super) fn install_macos_focus_listener() {
    use cocoa::{
        appkit::NSApplication,
        base::{id, nil},
        foundation::{NSAutoreleasePool, NSString},
    };
    use objc::{class, declare::ClassDecl, msg_send, sel, sel_impl};
    use objc::runtime::{Class, Object, Sel};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::Once;

    /// 应用失活时无条件隐藏临时窗口（切到其他应用）。
    extern "C" fn application_did_resign_active(
        _this: &Object,
        _cmd: Sel,
        _notification: id,
    ) {
        let _ = slint::invoke_from_event_loop(|| {
            hide_clipboard_popups();
            hide_quick_action_windows();
        });
    }

    /// 窗口失焦时：仅当失焦窗口本身是 popup/快捷窗口时才隐藏，避免主窗口 resignKey
    ///（显示 popup 时 makeKeyAndOrderFront 会让主窗口失焦）误伤刚显示的 popup。
    extern "C" fn window_did_resign_key(
        _this: &Object,
        _cmd: Sel,
        notification: id,
    ) {
        let resigned_window: id = unsafe { msg_send![notification, object] };
        if resigned_window == nil {
            return;
        }

        // 检查失焦窗口是否是某个 transient 窗口（popup 或 quick action）。
        let is_transient = |slint_window: &slint::Window| -> bool {
            let handle = slint_window.window_handle();
            let Ok(handle) = handle.window_handle() else {
                return false;
            };
            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                return false;
            };
            let ns_view = handle.ns_view.as_ptr() as id;
            let ns_window: id = unsafe { msg_send![ns_view, window] };
            ns_window == resigned_window
        };

        let should_hide = CLIPBOARD_WINDOWS.with(|windows| {
            windows.borrow().iter().any(|state| {
                state.window.get_popup_mode()
                    && state.window.window().is_visible()
                    && is_transient(&state.window.window())
            })
        }) || QUICK_WINDOWS.with(|windows| {
            windows.borrow().iter().any(|window| {
                window.window().is_visible() && is_transient(&window.window())
            })
        });

        if should_hide {
            let _ = slint::invoke_from_event_loop(|| {
                hide_clipboard_popups();
                hide_quick_action_windows();
            });
        }
    }

    // 仅注册一次自定义类，重复调用（如多次启动）时跳过。
    static REGISTER_CLASS: Once = Once::new();
    REGISTER_CLASS.call_once(|| {
        let superclass = class!(NSObject);
        if let Some(mut decl) = ClassDecl::new("DevToolsFocusObserver", superclass) {
            // applicationDidResignActive: 与 windowDidResignKey: 都是 NSNotification 回调签名。
            unsafe {
                decl.add_method(
                    sel!(applicationDidResignActive:),
                    application_did_resign_active as extern "C" fn(&Object, Sel, id),
                );
                decl.add_method(
                    sel!(windowDidResignKey:),
                    window_did_resign_key as extern "C" fn(&Object, Sel, id),
                );
            }
            decl.register();
        }
    });

    // observer 通过 new 持有 +1 retain，永不 release 让其随进程生命周期存活。
    // NSString 在 pool 内创建并发送给通知中心，中心会 retain 字符串，drain 后副本仍有效。
    // windowDidResignKey: 的 object 传 nil，监听应用内所有窗口失焦。
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
        let app: id = NSApplication::sharedApplication(nil);
        let observer_class = Class::get("DevToolsFocusObserver")
            .expect("DevToolsFocusObserver class not registered");
        let observer: id = msg_send![observer_class, new];

        let app_name =
            NSString::alloc(nil).init_str("NSApplicationDidResignActiveNotification");
        let _: () = msg_send![
            center,
            addObserver: observer
            selector: sel!(applicationDidResignActive:)
            name: app_name
            object: app
        ];

        let window_name =
            NSString::alloc(nil).init_str("NSWindowDidResignKeyNotification");
        let _: () = msg_send![
            center,
            addObserver: observer
            selector: sel!(windowDidResignKey:)
            name: window_name
            object: nil
        ];

        let _: () = msg_send![pool, drain];
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn install_macos_focus_listener() {}
