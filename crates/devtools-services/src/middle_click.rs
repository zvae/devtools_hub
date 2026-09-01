use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use arboard::Clipboard;
#[cfg(not(target_os = "macos"))]
use rdev::{listen, simulate, Button, Event, EventType, Key, SimulateError};
#[cfg(target_os = "macos")]
use rdev::{simulate, EventType, Key, SimulateError};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// 鼠标中键快捷窗口的显示/隐藏事件。
#[derive(Clone, Debug)]
pub enum QuickActionEvent {
    Show {
        selected_text: Option<String>,
        position: Option<ScreenPosition>,
    },
    Dismiss,
}

/// 屏幕坐标，供 UI 线程定位快捷窗口；macOS 为 Quartz 逻辑坐标，其它平台为物理坐标。
#[derive(Clone, Copy, Debug)]
pub struct ScreenPosition {
    pub x: i32,
    pub y: i32,
}

/// Controls whether the global middle-click action is active.
#[derive(Clone)]
pub struct MiddleClickController {
    enabled: Arc<AtomicBool>,
}

impl MiddleClickController {
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}

/// 启动全局中键监听线程。监听失败不会阻塞主程序启动。
pub fn spawn_middle_click_listener(
    tx: mpsc::UnboundedSender<QuickActionEvent>,
    initially_enabled: bool,
) -> MiddleClickController {
    let enabled = Arc::new(AtomicBool::new(initially_enabled));
    let (capture_tx, capture_rx) = std::sync::mpsc::channel::<Option<ScreenPosition>>();
    let dismiss_tx = tx.clone();
    let capture_enabled = Arc::clone(&enabled);

    thread::Builder::new()
        .name("selected-text-capture".into())
        .spawn(move || {
            while let Ok(position) = capture_rx.recv() {
                if !capture_enabled.load(Ordering::Relaxed) {
                    continue;
                }
                let selected_text = capture_selected_text();
                if !capture_enabled.load(Ordering::Relaxed) {
                    continue;
                }
                if tx
                    .send(QuickActionEvent::Show {
                        selected_text,
                        position,
                    })
                    .is_err()
                {
                    debug!("selected text capture stopped because receiver was dropped");
                    return;
                }
            }
        })
        .expect("failed to spawn selected text capture worker");

    #[cfg(target_os = "macos")]
    spawn_macos_middle_click_listener(capture_tx, dismiss_tx, Arc::clone(&enabled));

    #[cfg(not(target_os = "macos"))]
    spawn_rdev_middle_click_listener(capture_tx, dismiss_tx, Arc::clone(&enabled));

    MiddleClickController { enabled }
}

#[cfg(not(target_os = "macos"))]
fn spawn_rdev_middle_click_listener(
    capture_tx: std::sync::mpsc::Sender<Option<ScreenPosition>>,
    dismiss_tx: mpsc::UnboundedSender<QuickActionEvent>,
    enabled: Arc<AtomicBool>,
) {
    thread::Builder::new()
        .name("middle-click-listener".into())
        .spawn(move || {
            let mut cursor_position = None;
            let callback = move |event: Event| match event.event_type {
                EventType::ButtonRelease(Button::Middle) => {
                    if !enabled.load(Ordering::Relaxed) {
                        return;
                    }
                    let position = current_cursor_position().or(cursor_position);
                    if capture_tx.send(position).is_err() {
                        debug!("middle-click listener stopped because capture worker exited");
                    }
                }
                EventType::ButtonPress(button) if button != Button::Middle => {
                    if !enabled.load(Ordering::Relaxed) {
                        return;
                    }
                    schedule_external_dismiss(dismiss_tx.clone());
                }
                EventType::MouseMove { x, y } => {
                    cursor_position = Some(ScreenPosition {
                        x: x.round() as i32,
                        y: y.round() as i32,
                    });
                }
                _ => {}
            };

            if let Err(error) = listen(callback) {
                warn!(?error, "middle-click listener failed");
            }
        })
        .expect("failed to spawn middle-click listener");
}

#[cfg(target_os = "windows")]
fn current_cursor_position() -> Option<ScreenPosition> {
    use windows::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};

    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point).ok()? };
    Some(ScreenPosition {
        x: point.x,
        y: point.y,
    })
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn current_cursor_position() -> Option<ScreenPosition> {
    None
}

#[cfg(target_os = "macos")]
fn spawn_macos_middle_click_listener(
    capture_tx: std::sync::mpsc::Sender<Option<ScreenPosition>>,
    dismiss_tx: mpsc::UnboundedSender<QuickActionEvent>,
    enabled: Arc<AtomicBool>,
) {
    thread::Builder::new()
        .name("middle-click-listener".into())
        .spawn(move || {
            use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
            use core_graphics::event::{
                CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
                CGEventType, EventField,
            };
            use std::process;

            let tap = match CGEventTap::new(
                // The annotated session layer provides the process that will receive the event.
                CGEventTapLocation::AnnotatedSession,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                vec![
                    CGEventType::LeftMouseDown,
                    CGEventType::RightMouseDown,
                    CGEventType::OtherMouseDown,
                    CGEventType::OtherMouseUp,
                ],
                move |_proxy, event_type, event| {
                    let button =
                        event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);
                    if matches!(event_type, CGEventType::OtherMouseUp) && button == 2 {
                        if !enabled.load(Ordering::Relaxed) {
                            return None;
                        }
                        let location = event.location();
                        let position = Some(ScreenPosition {
                            x: location.x.round() as i32,
                            y: location.y.round() as i32,
                        });
                        if capture_tx.send(position).is_err() {
                            debug!("middle-click listener stopped because capture worker exited");
                        }
                    } else if matches!(
                        event_type,
                        CGEventType::LeftMouseDown
                            | CGEventType::RightMouseDown
                            | CGEventType::OtherMouseDown
                    ) && button != 2
                        && enabled.load(Ordering::Relaxed)
                        && {
                            let target_pid = event
                                .get_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID);
                            target_pid > 0 && target_pid != process::id() as i64
                        }
                    {
                        if dismiss_tx.send(QuickActionEvent::Dismiss).is_err() {
                            debug!("middle-click listener stopped because receiver was dropped");
                        }
                    }
                    None
                },
            ) {
                Ok(tap) => tap,
                Err(()) => {
                    warn!("macOS middle-click event tap could not be created");
                    return;
                }
            };

            let run_loop = CFRunLoop::get_current();
            let source = match tap.mach_port.create_runloop_source(0) {
                Ok(source) => source,
                Err(()) => {
                    warn!("macOS middle-click run-loop source could not be created");
                    return;
                }
            };
            let common_modes = unsafe { kCFRunLoopCommonModes };
            run_loop.add_source(&source, common_modes);
            tap.enable();
            CFRunLoop::run_current();
        })
        .expect("failed to spawn middle-click listener");
}

#[cfg(all(not(target_os = "macos"), target_os = "windows"))]
fn schedule_external_dismiss(tx: mpsc::UnboundedSender<QuickActionEvent>) {
    // The foreground window changes just after the low-level mouse callback. A short delay lets
    // GetForegroundWindow observe the window that actually received the click.
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };

        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return;
        }

        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut process_id as *mut u32));
        }
        if process_id != std::process::id() {
            let _ = tx.send(QuickActionEvent::Dismiss);
        }
    });
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn schedule_external_dismiss(tx: mpsc::UnboundedSender<QuickActionEvent>) {
    // rdev does not expose the target window on these platforms, so keep the listener limited to
    // the middle-click action rather than hiding a popup on clicks inside this application.
    let _ = tx;
}

/// 尝试读取当前选中文本：发送复制快捷键，读取剪贴板，然后恢复旧内容。
fn capture_selected_text() -> Option<String> {
    let mut clipboard = Clipboard::new().ok()?;
    let previous = clipboard.get_text().ok();

    send_copy_shortcut().ok()?;
    thread::sleep(Duration::from_millis(90));

    let selected = clipboard
        .get_text()
        .ok()
        .map(|text| text.trim_matches('\0').trim().to_string())
        .filter(|text| !text.is_empty());

    // Command/Ctrl+C does not change the clipboard when there is no selection. Avoid treating the
    // previous clipboard value as selected text in that case.
    let selected = match (previous.as_deref(), selected) {
        (Some(previous), Some(selected)) if previous == selected => None,
        (_, selected) => selected,
    };

    if let Some(previous) = previous {
        let _ = clipboard.set_text(previous);
    }

    selected
}

/// macOS 使用 Command+C 复制选中文本。
#[cfg(target_os = "macos")]
fn send_copy_shortcut() -> Result<(), SimulateError> {
    simulate(&EventType::KeyPress(Key::MetaLeft))?;
    thread::sleep(Duration::from_millis(20));
    simulate(&EventType::KeyPress(Key::KeyC))?;
    thread::sleep(Duration::from_millis(20));
    simulate(&EventType::KeyRelease(Key::KeyC))?;
    thread::sleep(Duration::from_millis(20));
    simulate(&EventType::KeyRelease(Key::MetaLeft))?;
    Ok(())
}

/// Windows/Linux 使用 Ctrl+C 复制选中文本。
#[cfg(not(target_os = "macos"))]
fn send_copy_shortcut() -> Result<(), SimulateError> {
    simulate(&EventType::KeyPress(Key::ControlLeft))?;
    thread::sleep(Duration::from_millis(20));
    simulate(&EventType::KeyPress(Key::KeyC))?;
    thread::sleep(Duration::from_millis(20));
    simulate(&EventType::KeyRelease(Key::KeyC))?;
    thread::sleep(Duration::from_millis(20));
    simulate(&EventType::KeyRelease(Key::ControlLeft))?;
    Ok(())
}
