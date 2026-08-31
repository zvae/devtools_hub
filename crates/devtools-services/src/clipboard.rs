use std::time::Duration;

use arboard::Clipboard;
use tokio::{sync::mpsc, time};
use tracing::{debug, warn};

use crate::platform::active_application_name;

#[derive(Clone, Debug)]
pub enum ClipboardEvent {
    TextChanged {
        content: String,
        source_app: Option<String>,
    },
}

pub async fn run_clipboard_watcher(tx: mpsc::UnboundedSender<ClipboardEvent>) {
    let mut clipboard = match Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            warn!(?error, "clipboard watcher could not start");
            return;
        }
    };

    let mut last_text = String::new();
    let mut interval = time::interval(Duration::from_millis(900));

    loop {
        interval.tick().await;

        match clipboard.get_text() {
            Ok(text) if text != last_text => {
                last_text = text.clone();
                if tx
                    .send(ClipboardEvent::TextChanged {
                        content: text,
                        source_app: active_application_name(),
                    })
                    .is_err()
                {
                    debug!("clipboard watcher stopped because receiver was dropped");
                    return;
                }
            }
            Ok(_) => {}
            Err(error) => {
                debug!(?error, "clipboard text unavailable");
            }
        }
    }
}

pub fn set_clipboard_text(text: &str) -> anyhow::Result<()> {
    let mut clipboard = Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}
