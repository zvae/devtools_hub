use std::time::Duration;

use arboard::Clipboard;
use tokio::{sync::mpsc, time};
use tracing::{debug, warn};

use crate::platform::active_application_name;

/// 剪贴板监听事件，目前只处理文本变化。
#[derive(Clone, Debug)]
pub enum ClipboardEvent {
    TextChanged {
        content: String,
        source_app: Option<String>,
    },
}

/// 轮询系统剪贴板。变化时同时尝试读取当前活动窗口标题作为复制来源。
pub async fn run_clipboard_watcher(tx: mpsc::UnboundedSender<ClipboardEvent>) {
    let mut clipboard = match Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            warn!(?error, "clipboard watcher could not start");
            return;
        }
    };

    let mut last_text = String::new();
    // 轮询间隔不宜太短，避免频繁访问系统剪贴板造成抖动。
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

/// 将文本写入系统剪贴板，工具输出和历史项恢复都会走这里。
pub fn set_clipboard_text(text: &str) -> anyhow::Result<()> {
    let mut clipboard = Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}
