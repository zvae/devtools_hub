//! 按键在界面上的写法：macOS 用 ⌘ ⇧ ↩ 等符号，Windows 用 Ctrl、Shift、Enter 等文字。

#[cfg(target_os = "macos")]
mod labels {
    pub const COMMAND: &str = "⌘";
    pub const SHIFT: &str = "⇧";
    pub const ALT: &str = "⌥";
    pub const RETURN: &str = "↩";
    pub const DELETE: &str = "⌫";
    pub const TAB: &str = "⇥";
    pub const ESCAPE: &str = "esc";
    /// 快捷键至少需要的修饰键。
    pub const MODIFIER_HINT: &str = "⌘ ⌥ ⌃ ⇧";
}

#[cfg(not(target_os = "macos"))]
mod labels {
    pub const COMMAND: &str = "Ctrl";
    pub const SHIFT: &str = "Shift";
    pub const ALT: &str = "Alt";
    pub const RETURN: &str = "Enter";
    pub const DELETE: &str = "Del";
    pub const TAB: &str = "Tab";
    pub const ESCAPE: &str = "Esc";
    pub const MODIFIER_HINT: &str = "Ctrl / Alt / Shift / Win";
}

pub use labels::*;

pub const SPACE: &str = "空格";

/// 组合键的文字写法：macOS 连写（`⇧⌘V`），Windows 用加号连接（`Ctrl+Shift+V`）。
pub fn shortcut(parts: &[&str]) -> String {
    if cfg!(target_os = "macos") {
        parts.concat()
    } else {
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_per_platform() {
        let text = shortcut(&[COMMAND, "V"]);
        if cfg!(target_os = "macos") {
            assert_eq!(text, "⌘V");
        } else {
            assert_eq!(text, "Ctrl+V");
        }
    }
}
