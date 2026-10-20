//! 截屏遮罩与贴图窗口的原生设置（Windows）。
//!
//! 两种窗口都由 egui 的多视口创建，这里按窗口标题在本线程的窗口里找到句柄再调整：
//! - 截屏遮罩：按物理像素铺满鼠标所在的显示器并置顶（winit 按逻辑坐标创建窗口，
//!   多个显示器缩放比例不同时位置不准），设为前台窗口接收按键；结束后把前台还给之前的窗口。
//! - 贴图：winit 的置顶窗口即可；点击贴图会让它成为前台窗口，与 Snipaste 一致。
//!
//! 贴图的不透明度在 Windows 上没有实现：分层窗口（`WS_EX_LAYERED`）与 wgpu 使用的 DXGI 翻转模型交换链不兼容。

use std::cell::Cell;
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GetForegroundWindow, GetWindowTextW, IsWindow, SetForegroundWindow,
    SetWindowPos, ShowWindow, ShowWindowAsync, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_HIDE,
};

use super::{panel, wide, workspace};

/// 贴图窗口是否支持调整不透明度。
pub const OPACITY_SUPPORTED: bool = false;

thread_local! {
    /// 截屏前的前台窗口，截屏结束后把前台还给它（只在主线程访问）。
    static PREVIOUS: Cell<HWND> = const { Cell::new(null_mut()) };
}

/// 正在显示的截屏遮罩的句柄（[`rescue_overlay`] 在其他线程上用到）。
static OVERLAY: AtomicIsize = AtomicIsize::new(0);

/// 按标题找到本线程（主线程，所有 winit 窗口都在这里创建）的窗口；找不到时为空。
pub(crate) fn window(title: &str) -> HWND {
    struct Search {
        title: Vec<u16>,
        found: HWND,
    }
    unsafe extern "system" fn check(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam as *mut Search);
        let mut buffer = [0u16; 256];
        let len = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        if len > 0 && buffer[..len as usize] == search.title[..] {
            search.found = hwnd;
            return 0;
        }
        1
    }
    let mut search = Search {
        title: title.encode_utf16().collect(),
        found: null_mut(),
    };
    // SAFETY: 回调只在 EnumThreadWindows 返回前被调用，lparam 指向本函数里的 search。
    unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(check),
            &mut search as *mut Search as LPARAM,
        );
    }
    search.found
}

/// 截屏遮罩出现后调用：按物理像素铺满显示器（`frame`）、置顶并设为前台。
/// 返回是否找到了窗口（窗口还没创建时下一帧再试）。
pub fn begin_overlay(title: &str, frame: [f64; 4]) -> bool {
    let hwnd = window(title);
    if hwnd.is_null() {
        return false;
    }
    let [x, y, width, height] = frame.map(|v| v as i32);
    // SAFETY: hwnd 是本线程的窗口。
    unsafe {
        let foreground = GetForegroundWindow();
        if workspace::is_other_app(foreground) {
            PREVIOUS.with(|cell| cell.set(foreground));
        }
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_SHOWWINDOW);
        panel::bring_to_front(hwnd);
    }
    OVERLAY.store(hwnd as isize, Ordering::Relaxed);
    true
}

/// 截屏遮罩的窗口编号，排除在滚动截图之外用（Windows 上还不支持滚动截图）。
pub fn overlay_window_id(_title: &str) -> Option<u32> {
    None
}

/// 收起截屏遮罩（egui 随后销毁窗口）。
pub fn hide_overlay(title: &str) {
    let hwnd = window(title);
    let _ = OVERLAY.compare_exchange(hwnd as isize, 0, Ordering::Relaxed, Ordering::Relaxed);
    if !hwnd.is_null() {
        // SAFETY: hwnd 是本线程的窗口。
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

/// 截屏遮罩卡住时由看门狗调用（任意线程）：收起遮罩。`ShowWindowAsync` 不等窗口所在的线程处理，
/// 那个线程卡住时也不会阻塞。前台留给用户点击别的窗口切换（其他线程不能可靠地设置前台窗口）。
pub fn rescue_overlay() {
    let hwnd = OVERLAY.swap(0, Ordering::Relaxed) as HWND;
    if !hwnd.is_null() {
        // SAFETY: 句柄可能已经失效，ShowWindowAsync 对无效句柄只返回失败。
        unsafe { ShowWindowAsync(hwnd, SW_HIDE) };
    }
}

/// 截屏结束：把前台还给截屏前的窗口（先还前台、稍后再收起遮罩，原因见 macOS 版）。
pub fn restore_focus() {
    let previous = PREVIOUS.with(|cell| cell.replace(null_mut()));
    // SAFETY: 句柄使用前检查是否仍然存在。
    unsafe {
        if !previous.is_null() && IsWindow(previous) != 0 {
            SetForegroundWindow(previous);
        }
    }
}

/// `SetForegroundWindow` 是同步的，还完就算恢复了。
pub fn focus_restored() -> bool {
    true
}

/// 贴图窗口不需要额外设置（置顶、不在任务栏显示都由 winit 完成）。
pub fn configure_pin(_title: &str) -> bool {
    true
}

/// 贴图窗口销毁前不需要处理（macOS 上要把换过的类换回去）。
pub fn release_pin(_title: &str) {}

/// 不支持（见模块说明）。
pub fn set_opacity(_title: &str, _opacity: f32) {}

/// 无边框窗口在 Windows 上没有系统阴影，不处理。
pub fn set_shadow(_title: &str, _shadow: bool) {}

/// winit 的 `drag_window` 在 Windows 上可靠，返回 false 交给它处理。
pub fn start_drag(_title: &str) -> bool {
    false
}

/// 一次设置窗口的位置和大小（`frame` 是逻辑坐标 `[x, y, 宽, 高]`），避免分成两次改变时窗口抖动。
/// 返回 false 表示没找到窗口。
pub fn set_window_frame(title: &str, frame: [f64; 4]) -> bool {
    let hwnd = window(title);
    if hwnd.is_null() {
        return false;
    }
    // SAFETY: hwnd 是本线程的有效窗口。
    unsafe {
        let scale = f64::from(GetDpiForWindow(hwnd).max(96)) / 96.0;
        let [x, y, width, height] = frame.map(|v| (v * scale).round() as i32);
        SetWindowPos(
            hwnd,
            null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        ) != 0
    }
}

/// 弹出保存对话框，返回选择的路径（自动补上 `.png`）；取消时返回 `None`。
pub fn save_dialog(file_name: &str, directory: Option<&Path>) -> Option<PathBuf> {
    let mut buffer = [0u16; 1024];
    let name: Vec<u16> = file_name.encode_utf16().take(buffer.len() - 1).collect();
    buffer[..name.len()].copy_from_slice(&name);
    // 过滤器是以两个 NUL 结尾的“说明\0模式\0”序列，`wide` 会补上最后一个 NUL。
    let filter = wide("PNG 图片\0*.png\0");
    let extension = wide("png");
    let directory = directory.map(|dir| wide(&dir.to_string_lossy()));
    // SAFETY: 结构体中的指针都指向本函数里的缓冲区，对话框返回前一直有效。
    unsafe {
        let mut dialog: OPENFILENAMEW = zeroed();
        dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrFile = buffer.as_mut_ptr();
        dialog.nMaxFile = buffer.len() as u32;
        dialog.lpstrDefExt = extension.as_ptr();
        dialog.lpstrInitialDir = directory.as_ref().map_or(null(), |dir| dir.as_ptr());
        dialog.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        if GetSaveFileNameW(&mut dialog) == 0 {
            return None;
        }
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(crate::snip::ensure_png(PathBuf::from(
        String::from_utf16_lossy(&buffer[..len]),
    )))
}
