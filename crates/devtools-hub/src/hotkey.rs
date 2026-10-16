//! 全局快捷键。
//!
//! PasteNow 的 `CarbonShortcutManager` 调用 Carbon 的 `RegisterEventHotKey`；
//! global-hotkey 在 macOS 上用的是同一个 API，在 Windows 上用 `RegisterHotKey`，都不需要额外权限。

use std::str::FromStr;
use std::sync::Arc;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::controller;
use crate::keys;
use crate::shared::Shared;

pub struct HotkeyService {
    manager: GlobalHotKeyManager,
    current: Option<HotKey>,
}

impl HotkeyService {
    /// 必须在主线程创建。
    pub fn new(shared: Arc<Shared>) -> anyhow::Result<Self> {
        let manager = GlobalHotKeyManager::new()?;
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state() == HotKeyState::Pressed {
                controller::toggle_panel(&shared);
            }
        }));
        Ok(Self {
            manager,
            current: None,
        })
    }

    /// 暂时注销当前快捷键（录制新快捷键时使用），之后调用 [`Self::apply`] 恢复。
    pub fn suspend(&mut self) {
        if let Some(old) = self.current.take() {
            let _ = self.manager.unregister(old);
        }
    }

    /// 注册新的快捷键，失败时保留原来的注册。
    pub fn apply(&mut self, spec: &str) -> Result<(), String> {
        let hotkey = parse(spec)?;
        if self.current == Some(hotkey) {
            return Ok(());
        }
        if let Some(old) = self.current.take() {
            let _ = self.manager.unregister(old);
        }
        match self.manager.register(hotkey) {
            Ok(()) => {
                self.current = Some(hotkey);
                Ok(())
            }
            Err(err) => Err(format!(
                "注册快捷键 {spec} 失败：{err}（可能已被其他应用占用）"
            )),
        }
    }
}

pub fn parse(spec: &str) -> Result<HotKey, String> {
    let hotkey = HotKey::from_str(spec).map_err(|err| format!("快捷键格式不正确：{err}"))?;
    if hotkey.mods.is_empty() {
        return Err(format!(
            "快捷键至少需要一个修饰键（{}）",
            keys::MODIFIER_HINT
        ));
    }
    Ok(hotkey)
}

/// 修饰键的显示顺序与写法（global-hotkey 中的名称 → 界面上的写法）。
/// macOS 按 ⌃⌥⇧⌘ 的惯例排序；Windows 按 Ctrl+Shift+Alt+Win，且 `CmdOrCtrl` 表示 Ctrl。
#[cfg(target_os = "macos")]
const MODIFIERS: [(&str, &[&str]); 4] = [
    ("⌃", &["CTRL", "CONTROL"]),
    ("⌥", &["ALT", "OPTION"]),
    ("⇧", &["SHIFT"]),
    (
        "⌘",
        &["CMD", "COMMAND", "SUPER", "CMDORCTRL", "COMMANDORCONTROL"],
    ),
];
#[cfg(not(target_os = "macos"))]
const MODIFIERS: [(&str, &[&str]); 4] = [
    (
        "Ctrl",
        &["CTRL", "CONTROL", "CMDORCTRL", "COMMANDORCONTROL"],
    ),
    ("Shift", &["SHIFT"]),
    ("Alt", &["ALT", "OPTION"]),
    ("Win", &["CMD", "COMMAND", "SUPER"]),
];

/// macOS：`Cmd+Shift+V` → `⇧⌘V`；Windows：`Alt+Shift+V` → `Shift+Alt+V`
pub fn display(spec: &str) -> String {
    let parts = display_parts(spec);
    keys::shortcut(&parts.iter().map(String::as_str).collect::<Vec<_>>())
}

/// 拆成一个个按键，用于绘制键帽：`Cmd+Shift+V` → `["⇧", "⌘", "V"]`。
pub fn display_parts(spec: &str) -> Vec<String> {
    let parts: Vec<String> = spec
        .split('+')
        .map(|p| p.trim().to_ascii_uppercase())
        .collect();
    let mut out = Vec::new();
    for (symbol, names) in MODIFIERS {
        if parts.iter().any(|p| names.contains(&p.as_str())) {
            out.push(symbol.to_string());
        }
    }
    if let Some(last) = parts.last() {
        let key = last
            .trim_start_matches("KEY")
            .trim_start_matches("DIGIT")
            .replace("SPACE", "空格")
            .replace("ARROW", "");
        out.push(key);
    }
    out
}

/// 把 egui 捕获到的按键转成 global-hotkey 可解析的字符串（快捷键录制用）。
pub fn from_egui(modifiers: egui::Modifiers, key: egui::Key) -> Option<String> {
    let name = key.name();
    let is_function_key =
        name.len() > 1 && name.starts_with('F') && name[1..].parse::<u8>().is_ok();
    let key_part = match key {
        egui::Key::Space | egui::Key::Enter | egui::Key::Tab => name,
        _ if name.chars().count() == 1 || is_function_key => name,
        _ => return None,
    };
    let mut parts = Vec::new();
    if modifiers.ctrl {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    // 只看 mac_cmd：在 Windows 上 egui 的 `command` 等于 Ctrl，不能当成 Win 键。
    if modifiers.mac_cmd {
        parts.push("Cmd");
    }
    if parts.is_empty() {
        return None;
    }
    let spec = format!("{}+{key_part}", parts.join("+"));
    parse(&spec).ok().map(|_| spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_and_rejects_bare_keys() {
        assert!(parse("Cmd+Shift+V").is_ok());
        assert!(parse("Ctrl+Alt+Space").is_ok());
        assert!(parse("V").is_err());
        assert!(parse("Cmd+Nope").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn displays_symbols() {
        assert_eq!(display("Cmd+Shift+V"), "⇧⌘V");
        assert_eq!(display("Ctrl+Alt+Space"), "⌃⌥空格");
        assert_eq!(display_parts("Shift+Cmd+V"), ["⇧", "⌘", "V"]);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn displays_words() {
        assert_eq!(display("Alt+Shift+V"), "Shift+Alt+V");
        assert_eq!(display("CmdOrCtrl+Space"), "Ctrl+空格");
        assert_eq!(display_parts("Ctrl+Super+K"), ["Ctrl", "Win", "K"]);
    }

    #[test]
    fn converts_egui_keys() {
        let mods = egui::Modifiers {
            mac_cmd: true,
            command: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            from_egui(mods, egui::Key::V).as_deref(),
            Some("Shift+Cmd+V")
        );
        assert_eq!(from_egui(egui::Modifiers::NONE, egui::Key::V), None);
        // Windows 上按 Ctrl：egui 的 command 也为 true，但不应被当成 Cmd/Win。
        let ctrl = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        assert_eq!(from_egui(ctrl, egui::Key::K).as_deref(), Some("Ctrl+K"));
    }
}
