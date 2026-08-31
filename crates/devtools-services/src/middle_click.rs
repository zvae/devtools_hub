use std::{thread, time::Duration};

use arboard::Clipboard;
use rdev::{listen, simulate, Button, Event, EventType, Key, SimulateError};
use tokio::sync::mpsc;
use tracing::{debug, warn};

#[derive(Clone, Debug)]
pub struct QuickActionEvent {
    pub selected_text: Option<String>,
}

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

#[cfg(target_os = "macos")]
fn send_copy_shortcut() -> Result<(), SimulateError> {
    simulate(&EventType::KeyPress(Key::MetaLeft))?;
    simulate(&EventType::KeyPress(Key::KeyC))?;
    simulate(&EventType::KeyRelease(Key::KeyC))?;
    simulate(&EventType::KeyRelease(Key::MetaLeft))?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn send_copy_shortcut() -> Result<(), SimulateError> {
    simulate(&EventType::KeyPress(Key::ControlLeft))?;
    simulate(&EventType::KeyPress(Key::KeyC))?;
    simulate(&EventType::KeyRelease(Key::KeyC))?;
    simulate(&EventType::KeyRelease(Key::ControlLeft))?;
    Ok(())
}
