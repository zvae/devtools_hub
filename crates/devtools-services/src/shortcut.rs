use std::{thread, time::Duration};

use anyhow::Result;
use crossbeam_channel::RecvTimeoutError;
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// 全局快捷键事件。当前只有显示/隐藏主窗口一种动作。
#[derive(Clone, Debug)]
pub enum ShortcutEvent {
    ToggleWindow,
}

/// Owns the platform hotkey manager on the thread where it was created.
pub struct GlobalShortcut {
    manager: GlobalHotKeyManager,
    hotkey: HotKey,
}

/// 注册全局快捷键并启动监听线程。
pub fn spawn_global_shortcut_listener(
    tx: mpsc::UnboundedSender<ShortcutEvent>,
    binding: Option<&str>,
) -> Result<GlobalShortcut> {
    let manager = GlobalHotKeyManager::new()?;
    let hotkey = parse_hotkey(binding.unwrap_or("Alt+Space"))?;
    manager.register(hotkey)?;

    thread::Builder::new()
        .name("global-shortcut-listener".into())
        .spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            loop {
                match receiver.recv_timeout(Duration::from_millis(250)) {
                    Ok(event) if event.state == HotKeyState::Pressed => {
                        if tx.send(ShortcutEvent::ToggleWindow).is_err() {
                            debug!("shortcut listener stopped because receiver was dropped");
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(error) => {
                        warn!(?error, "global hotkey listener failed");
                        return;
                    }
                }
            }
        })?;

    Ok(GlobalShortcut { manager, hotkey })
}

/// Replace the registered shortcut without restarting the application.
pub fn update_global_shortcut(shortcut: &mut GlobalShortcut, binding: &str) -> Result<()> {
    let new_hotkey = parse_hotkey(binding)?;
    if new_hotkey == shortcut.hotkey {
        return Ok(());
    }

    shortcut.manager.register(new_hotkey)?;
    if let Err(error) = shortcut.manager.unregister(shortcut.hotkey) {
        let _ = shortcut.manager.unregister(new_hotkey);
        return Err(error.into());
    }
    shortcut.hotkey = new_hotkey;
    Ok(())
}

/// Parse the canonical shortcut names emitted by the settings key recorder.
fn parse_hotkey(binding: &str) -> Result<HotKey> {
    let binding = binding.trim();
    if binding.is_empty() {
        anyhow::bail!("shortcut cannot be empty")
    }

    binding
        .parse::<HotKey>()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::parse_hotkey;
    use global_hotkey::hotkey::{Code, Modifiers};

    #[test]
    fn parses_shortcuts_emitted_by_the_key_recorder() {
        let hotkey = parse_hotkey("Ctrl+Shift+K").expect("shortcut should parse");
        assert_eq!(hotkey.key, Code::KeyK);
        assert_eq!(hotkey.mods, Modifiers::CONTROL | Modifiers::SHIFT);
    }

    #[test]
    fn rejects_empty_shortcuts() {
        assert!(parse_hotkey(" ").is_err());
    }
}
