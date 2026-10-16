//! 浮动面板窗口控制（Windows）。
//!
//! 与 macOS 版相同的思路：弹出时记住之前的前台窗口并把面板设为前台，
//! 粘贴前隐藏面板、把焦点还给之前的窗口，再由调用方发送 Ctrl+V。
//! 点击托盘图标时，面板贴着任务栏出现在图标旁边（任务栏在下方时出现在图标上方）。

use std::cell::Cell;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use anyhow::{bail, Result};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTONULL,
};
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetCursorPos, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
    IsWindow, IsWindowVisible, SetForegroundWindow, SetWindowPos, ShowWindow, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
};

use super::workspace;
pub use crate::platform::{Anchor, Placement};

/// egui 背景画成直角，圆角由 DWM 处理（Windows 11 上为系统圆角，Windows 10 为直角）。
pub const CORNER_RADIUS: f64 = 0.0;

thread_local! {
    static PANEL: Cell<HWND> = const { Cell::new(null_mut()) };
    static PREVIOUS: Cell<HWND> = const { Cell::new(null_mut()) };
}

/// 面板窗口句柄；不在主线程或尚未绑定时为空。
pub(crate) fn window() -> HWND {
    PANEL.with(Cell::get)
}

/// 绑定 eframe 创建的窗口。必须在主线程调用。
///
/// `start_hidden`：eframe 会在第一帧之后强制显示窗口，先把窗口移到屏幕外，
/// 之后由 [`finish_startup_hide`] 隐藏，避免启动时闪出一个面板。
pub fn attach(handle: &impl HasWindowHandle, start_hidden: bool) -> Result<()> {
    let RawWindowHandle::Win32(raw) = handle.window_handle()?.as_raw() else {
        bail!("不是 Win32 窗口");
    };
    let hwnd = raw.hwnd.get() as HWND;
    let preference = DWMWCP_ROUND;
    // SAFETY: hwnd 来自 winit；属性值为 4 字节整数。Windows 10 不支持该属性，调用失败无影响。
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&preference as *const i32).cast(),
            size_of::<i32>() as u32,
        );
        if start_hidden {
            SetWindowPos(
                hwnd,
                null_mut(),
                -32000,
                -32000,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    PANEL.with(|panel| panel.set(hwnd));
    tracing::debug!("已绑定面板窗口 {hwnd:?}");
    Ok(())
}

/// 收起启动时被 eframe 显示出来的（屏幕外的）窗口。
pub fn finish_startup_hide() {
    let hwnd = window();
    if !hwnd.is_null() {
        // SAFETY: hwnd 是已绑定的面板窗口。
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

/// Windows 上不使用毛玻璃背景（wgpu 的透明窗口在 Windows 上支持不稳定）。
pub fn has_vibrancy() -> bool {
    false
}

pub fn is_visible() -> bool {
    let hwnd = window();
    // SAFETY: 只读查询。
    !hwnd.is_null() && unsafe { IsWindowVisible(hwnd) } != 0
}

/// 拖动面板：Windows 上 winit 的 `drag_window` 可靠，返回 false 交给它处理。
pub fn start_drag() -> bool {
    false
}

/// 面板正在显示且处于前台。
pub fn is_active() -> bool {
    let hwnd = window();
    // SAFETY: 只读查询。
    is_visible() && unsafe { GetForegroundWindow() } == hwnd
}

pub fn show(anchor: Anchor) -> bool {
    let hwnd = window();
    if hwnd.is_null() {
        return false;
    }
    // SAFETY: hwnd 是已绑定的面板窗口。
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            // 从托盘弹出时前台是任务栏（或我们自己的托盘窗口），它不能作为粘贴目标；
            // 这种情况回退到监听线程最近记录的真实前台程序。
            let foreground = GetForegroundWindow();
            let previous = if workspace::is_other_app(foreground) {
                foreground
            } else {
                workspace::last_other_foreground()
            };
            if !previous.is_null() && IsWindow(previous) != 0 {
                PREVIOUS.with(|cell| cell.set(previous));
            }
            position(hwnd, anchor);
        }
        ShowWindow(hwnd, SW_SHOW);
        bring_to_front(hwnd);
    }
    tracing::debug!(
        "显示面板：{anchor:?}，之前的前台程序 {:?}",
        previous_app_name()
    );
    true
}

/// 隐藏面板；`restore_focus` 为 true 时激活弹出面板之前的前台窗口。
pub fn hide(restore_focus: bool) {
    let hwnd = window();
    if hwnd.is_null() {
        return;
    }
    let previous = PREVIOUS.with(|previous| previous.replace(null_mut()));
    // SAFETY: hwnd 是已绑定的面板窗口；previous 使用前检查是否仍然存在。
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
        if restore_focus && !previous.is_null() && IsWindow(previous) != 0 {
            SetForegroundWindow(previous);
        }
    }
}

/// 之前的前台程序名称（右键菜单显示“粘贴到 xxx”）。
pub fn previous_app_name() -> Option<String> {
    let previous = PREVIOUS.with(Cell::get);
    (!previous.is_null())
        .then(|| workspace::describe_window(previous).name)
        .flatten()
}

/// 后台程序直接调用 SetForegroundWindow 会被系统拦截（只会闪烁任务栏按钮），
/// 临时把本线程的输入队列挂到当前前台线程上即可获准。
unsafe fn bring_to_front(hwnd: HWND) {
    let foreground = GetForegroundWindow();
    let foreground_thread = GetWindowThreadProcessId(foreground, null_mut());
    let current_thread = GetCurrentThreadId();
    let attached = foreground_thread != 0
        && foreground_thread != current_thread
        && AttachThreadInput(current_thread, foreground_thread, 1) != 0;
    BringWindowToTop(hwnd);
    SetForegroundWindow(hwnd);
    SetFocus(hwnd);
    if attached {
        AttachThreadInput(current_thread, foreground_thread, 0);
    }
}

unsafe fn position(hwnd: HWND, anchor: Anchor) {
    let mut frame: RECT = zeroed();
    if GetWindowRect(hwnd, &mut frame) == 0 {
        return;
    }
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    let cursor = cursor_position();
    let (x, y, work) = match anchor {
        Anchor::Placement(Placement::NearCursor) => {
            (cursor.x + 6, cursor.y + 6, work_area_at(cursor))
        }
        Anchor::Placement(Placement::ScreenCenter) => {
            let work = work_area_at(cursor);
            let (work_width, work_height) = (work.right - work.left, work.bottom - work.top);
            (
                work.left + (work_width - width) / 2,
                work.top + (work_height - height) / 2 - work_height / 12,
                work,
            )
        }
        Anchor::Placement(Placement::LastPosition) => {
            // 上次的位置仍在某个显示器上就不动，否则居中（例如外接显示器已断开）。
            if !MonitorFromRect(&frame, MONITOR_DEFAULTTONULL).is_null() {
                return;
            }
            return position(hwnd, Anchor::Placement(Placement::ScreenCenter));
        }
        Anchor::StatusItem(tray) => {
            let icon = tray.map_or(
                RECT {
                    left: cursor.x,
                    top: cursor.y,
                    right: cursor.x + 1,
                    bottom: cursor.y + 1,
                },
                |tray| RECT {
                    left: tray.x as i32,
                    top: tray.y as i32,
                    right: (tray.x + tray.width) as i32,
                    bottom: (tray.y + tray.height) as i32,
                },
            );
            let center = POINT {
                x: (icon.left + icon.right) / 2,
                y: (icon.top + icon.bottom) / 2,
            };
            let work = work_area_at(center);
            // 任务栏在下方时出现在图标上方，在上方时出现在图标下方；左右两侧由下面的约束贴边。
            let y = if center.y >= (work.top + work.bottom) / 2 {
                work.bottom.min(icon.top) - height - 8
            } else {
                work.top.max(icon.bottom) + 8
            };
            (center.x - width / 2, y, work)
        }
    };
    let x = x.clamp(work.left + 4, (work.right - width - 4).max(work.left));
    let y = y.clamp(work.top + 4, (work.bottom - height - 4).max(work.top));
    SetWindowPos(
        hwnd,
        null_mut(),
        x,
        y,
        0,
        0,
        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

unsafe fn cursor_position() -> POINT {
    let mut point = POINT { x: 0, y: 0 };
    GetCursorPos(&mut point);
    point
}

/// 点所在显示器的工作区（扣除任务栏）。
unsafe fn work_area_at(point: POINT) -> RECT {
    let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(monitor, &mut info);
    info.rcWork
}
