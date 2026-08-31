use std::{thread, time::Duration};

use arboard::Clipboard;
use rdev::{listen, simulate, Button, Event, EventType, Key, SimulateError};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// 鼠标中键触发的快捷动作事件，可携带当前选中文本。
#[derive(Clone, Debug)]
pub struct QuickActionEvent {
    pub selected_text: Option<String>,
}

/// 启动全局中键监听线程。监听失败不会阻塞主程序启动。
pub fn spawn_middle_click_listener(tx: mpsc::UnboundedSender<QuickActionEvent>) {
    thread::Builder::new()
        .name("middle-click-listener".into())
        .spawn(move || {
            let callback = move |event: Event| {
                if matches!(event.event_type, EventType::ButtonRelease(Button::Middle)) {
                    let selected_text = capture_selected_text();
                    if tx.send(QuickActionEvent { selected_text }).is_err() {
                        debug!("middle-click listener stopped because receiver was dropped");
                    }
                }
            };

            if let Err(error) = listen(callback) {
                warn!(?error, "middle-click listener failed");
            }
        })
        .expect("failed to spawn middle-click listener");
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

    if let Some(previous) = previous {
        let _ = clipboard.set_text(previous);
    }

    selected
}

/// macOS 使用 Command+C 复制选中文本。
#[cfg(target_os = "macos")]
fn send_copy_shortcut() -> Result<(), SimulateError> {
    simulate(&EventType::KeyPress(Key::MetaLeft))?;
    simulate(&EventType::KeyPress(Key::KeyC))?;
    simulate(&EventType::KeyRelease(Key::KeyC))?;
    simulate(&EventType::KeyRelease(Key::MetaLeft))?;
    Ok(())
}

/// Windows/Linux 使用 Ctrl+C 复制选中文本。
#[cfg(not(target_os = "macos"))]
fn send_copy_shortcut() -> Result<(), SimulateError> {
    simulate(&EventType::KeyPress(Key::ControlLeft))?;
    simulate(&EventType::KeyPress(Key::KeyC))?;
    simulate(&EventType::KeyRelease(Key::KeyC))?;
    simulate(&EventType::KeyRelease(Key::ControlLeft))?;
    Ok(())
}
