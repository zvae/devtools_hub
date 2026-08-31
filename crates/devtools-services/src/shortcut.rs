use std::{thread, time::Duration};

use anyhow::Result;
use crossbeam_channel::RecvTimeoutError;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use tokio::sync::mpsc;
use tracing::{debug, warn};

#[derive(Clone, Debug)]
pub enum ShortcutEvent {
    ToggleWindow,
}

pub fn spawn_global_shortcut_listener(
    tx: mpsc::UnboundedSender<ShortcutEvent>,
    binding: Option<&str>,
) -> Result<()> {
    let manager = GlobalHotKeyManager::new()?;
    let hotkey = parse_hotkey(binding.unwrap_or("Alt+Space"));
    manager.register(hotkey)?;

    thread::Builder::new()
        .name("global-shortcut-listener".into())
        .spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            loop {
                match receiver.recv_timeout(Duration::from_millis(250)) {
                    Ok(_) => {
                        if tx.send(ShortcutEvent::ToggleWindow).is_err() {
                            debug!("shortcut listener stopped because receiver was dropped");
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(error) => {
                        warn!(?error, "global hotkey listener failed");
                        return;
                    }
                }
            }
        })?;

    // Keep the manager alive for the process lifetime. Phase 1 can replace this with
    // an owned service that supports unregistering and hotkey changes.
    Box::leak(Box::new(manager));
    Ok(())
}

#[cfg(target_os = "macos")]
fn default_modifier() -> Modifiers {
    Modifiers::ALT
}

fn parse_hotkey(binding: &str) -> HotKey {
    let mut modifiers = Modifiers::empty();
    let mut code = Code::Space;

    for part in binding
        .split('+')
        .map(|part| part.trim().to_ascii_lowercase())
    {
        match part.as_str() {
            "alt" | "option" => modifiers |= Modifiers::ALT,
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            "shift" => modifiers |= Modifiers::SHIFT,
            "cmd" | "command" | "meta" | "super" => modifiers |= Modifiers::SUPER,
            "space" => code = Code::Space,
            "enter" | "return" => code = Code::Enter,
            "j" => code = Code::KeyJ,
            "k" => code = Code::KeyK,
            "p" => code = Code::KeyP,
            _ => {}
        }
    }

    if modifiers.is_empty() {
        modifiers = default_modifier();
    }

    HotKey::new(Some(modifiers), code)
}

#[cfg(not(target_os = "macos"))]
fn default_modifier() -> Modifiers {
    Modifiers::ALT
}
