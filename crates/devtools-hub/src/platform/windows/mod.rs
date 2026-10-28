//! Windows 平台能力：剪贴板、来源程序、模拟 Ctrl+V / Ctrl+C、鼠标中键监听、窗口控制、右键菜单、程序图标、开机启动、安装更新、
//! 截屏、贴图窗口与文字识别。
//!
//! 与 `platform/macos` 提供相同的模块和函数；剪贴板格式与 UTI 的转换在
//! `devtools_clipboard::windows_formats` 中，这里只负责调用 Win32 API。

pub mod floating;
pub mod icons;
pub mod keyboard;
pub mod login_item;
pub mod menu;
pub mod mouse;
// windows crate 的 WinRT 部分（windows-future）只能在 Windows 上编译，在别的平台上类型检查时跳过。
#[cfg(windows)]
pub mod ocr;
pub mod panel;
pub mod pasteboard;
mod resize;
pub mod screen;
mod scroll_mask;
pub mod updater;
pub mod workspace;

use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSMICON;

/// UTF-8 → 以 NUL 结尾的 UTF-16（Win32 的 `PCWSTR`）。
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 系统 DPI 下小图标（托盘图标）的边长，单位像素：100% 缩放时为 16。
pub fn small_icon_size() -> u32 {
    // SAFETY: 只读取系统度量，没有指针参数。
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) };
    u32::try_from(size)
        .ok()
        .filter(|&size| size > 0)
        .unwrap_or(16)
}
