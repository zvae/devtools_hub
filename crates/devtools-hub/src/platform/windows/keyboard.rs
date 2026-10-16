//! 模拟 Ctrl+V 粘贴。
//!
//! Windows 上用 `SendInput` 发送按键，不需要额外授权（macOS 需要“辅助功能”权限）。
//! 注意：系统不允许普通权限的程序向以管理员身份运行的窗口发送按键（UIPI），
//! 这种情况下内容仍会写入剪贴板，用户可以手动粘贴。

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

/// `V` 的虚拟键码。Windows 程序按虚拟键码识别 Ctrl+V，与键盘布局无关。
const VK_V: u16 = 0x56;

/// Windows 模拟按键不需要授权，始终返回 true。
pub fn accessibility_trusted(_prompt: bool) -> bool {
    true
}

/// Windows 没有对应的设置页。
pub fn open_accessibility_settings() {}

pub fn paste_keycode() -> u16 {
    VK_V
}

/// 向前台窗口发送 Ctrl+V。可以在任意线程调用。
pub fn post_paste(keycode: u16) {
    let mut inputs = Vec::with_capacity(8);
    // 用户可能还按着 Shift（⇧↩ 以纯文本粘贴）或 Alt/Win，先松开，避免变成别的组合键。
    for modifier in [VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN] {
        // SAFETY: 只读取按键状态。
        if unsafe { GetAsyncKeyState(i32::from(modifier)) } < 0 {
            inputs.push(key(modifier, true));
        }
    }
    inputs.extend([
        key(VK_CONTROL, false),
        key(keycode, false),
        key(keycode, true),
        key(VK_CONTROL, true),
    ]);
    // SAFETY: inputs 是长度正确的 INPUT 数组。
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent as usize != inputs.len() {
        tracing::warn!("模拟 Ctrl+V 失败（目标窗口可能以管理员身份运行）");
    }
}

fn key(virtual_key: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
