use std::{thread, time::Duration};

use anyhow::Result;
use crossbeam_channel::RecvTimeoutError;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// 全局快捷键事件。当前只有显示/隐藏主窗口一种动作。
#[derive(Clone, Debug)]
pub enum ShortcutEvent {
    ToggleWindow,
}

/// 注册全局快捷键并启动监听线程。
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

    // 全局快捷键管理器需要常驻进程生命周期，后续可替换为支持注销/热更新的服务。
    Box::leak(Box::new(manager));
    Ok(())
}

/// macOS 默认修饰键也先使用 Alt/Option，和 README 中的说明保持一致。
#[cfg(target_os = "macos")]
fn default_modifier() -> Modifiers {
    Modifiers::ALT
}

/// 解析形如 Alt+Space 的快捷键配置，无法识别的按键片段会被忽略。
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

/// Windows/Linux 默认使用 Alt 作为全局唤起修饰键。
#[cfg(not(target_os = "macos"))]
fn default_modifier() -> Modifiers {
    Modifiers::ALT
}
