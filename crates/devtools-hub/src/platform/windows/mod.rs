//! Windows 平台能力：剪贴板、来源程序、模拟 Ctrl+V / Ctrl+C、鼠标中键监听、窗口控制、程序图标、开机启动。
//!
//! 与 `platform/macos` 提供相同的模块和函数；剪贴板格式与 UTI 的转换在
//! `devtools_clipboard::windows_formats` 中，这里只负责调用 Win32 API。

pub mod icons;
pub mod keyboard;
pub mod login_item;
pub mod mouse;
pub mod panel;
pub mod pasteboard;
pub mod workspace;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

/// UTF-8 → 以 NUL 结尾的 UTF-16（Win32 的 `PCWSTR`）。
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 读取 `HKEY_CURRENT_USER` 下的 DWORD 值。
pub(crate) fn read_user_dword(subkey: &str, value: &str) -> Option<u32> {
    let (subkey, value) = (wide(subkey), wide(value));
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: 字符串以 NUL 结尾；data/size 指向有效的栈变量。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}

/// 任务栏是否为浅色主题（决定托盘图标画成黑色还是白色）。
pub fn taskbar_uses_light_theme() -> bool {
    read_user_dword(
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
        "SystemUsesLightTheme",
    ) == Some(1)
}
