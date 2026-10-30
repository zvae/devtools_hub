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
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowRect,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, SetForegroundWindow, SetWindowPos,
    ShowWindow, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
    SW_SHOWNOACTIVATE,
};

use super::workspace;
pub use crate::platform::{Anchor, Backdrop, Placement};

/// egui 背景画成直角，圆角由 DWM 处理（Windows 11 上为系统圆角，Windows 10 为直角）。
pub const CORNER_RADIUS: f64 = 0.0;

thread_local! {
    /// 面板窗口与弹出面板前的前台窗口（收起时把焦点还给它），只在主线程访问。
    static PANEL: Cell<HWND> = const { Cell::new(null_mut()) };
    static PREVIOUS: Cell<HWND> = const { Cell::new(null_mut()) };
    /// 被 [`wake`] 临时摆到屏幕外时，面板原来的位置。
    static WAKING: Cell<Option<RECT>> = const { Cell::new(None) };
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
    // 无边框窗口在 Windows 上没有可以拖动的边框，四边和四个角都不出现缩放鼠标（见 `resize`）。
    super::resize::add_borders(hwnd);
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
pub fn backdrop() -> Backdrop {
    Backdrop::Opaque
}

/// 面板窗口是否可见（不论是否有焦点）。为截屏、贴图临时摆出的窗口不算（见 [`wake`]）。
pub fn is_visible() -> bool {
    let hwnd = window();
    // SAFETY: 只读查询。
    !is_waking() && !hwnd.is_null() && unsafe { IsWindowVisible(hwnd) } != 0
}

/// 让 eframe 运行一次界面（原因见 macOS 版的同名函数）：把面板显示在屏幕外、不激活，
/// 新窗口创建后由 [`finish_wake`] 隐藏并放回原处。
pub fn wake() {
    let hwnd = window();
    // SAFETY: hwnd 是已绑定的面板窗口。
    unsafe {
        if hwnd.is_null() || IsWindowVisible(hwnd) != 0 {
            return;
        }
        let mut frame: RECT = zeroed();
        GetWindowRect(hwnd, &mut frame);
        WAKING.with(|cell| cell.set(Some(frame)));
        SetWindowPos(
            hwnd,
            null_mut(),
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

/// 面板是否正被 [`wake`] 临时摆在屏幕外（这时不用绘制面板内容）。
pub fn is_waking() -> bool {
    WAKING.with(Cell::get).is_some()
}

/// 隐藏 [`wake`] 摆出的窗口并放回原处。
pub fn finish_wake() {
    let hwnd = window();
    if hwnd.is_null() {
        return;
    }
    // SAFETY: hwnd 是已绑定的面板窗口。
    unsafe {
        if is_waking() {
            ShowWindow(hwnd, SW_HIDE);
        }
        end_wake(hwnd);
    }
}

/// 结束临时显示：把面板放回原来的位置（不改变显示状态）。
unsafe fn end_wake(hwnd: HWND) {
    if let Some(frame) = WAKING.with(|cell| cell.take()) {
        SetWindowPos(
            hwnd,
            null_mut(),
            frame.left,
            frame.top,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
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

/// 显示面板并设为前台。`size` 是内容区大小（逻辑像素）：同一个窗口既用作剪贴板面板，
/// 也用作鼠标中键快捷面板和 JSON 编辑器，各自大小不同。
/// 窗口原本隐藏、或者大小有变化时，按 `anchor` 重新定位。
pub fn show(anchor: Anchor, size: [f32; 2]) -> bool {
    let hwnd = window();
    if hwnd.is_null() {
        return false;
    }
    // SAFETY: hwnd 是已绑定的面板窗口。
    unsafe {
        // 正被临时摆在屏幕外（见 `wake`）时按隐藏处理：放回原处后重新定位。
        let waking = is_waking();
        end_wake(hwnd);
        let visible = IsWindowVisible(hwnd) != 0 && !waking;
        if !visible {
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
        }
        if resize(hwnd, size) || !visible {
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

/// 让面板重新成为前台窗口，返回是否成功（面板没显示时失败）。
///
/// 系统的保存对话框关掉之后会把焦点还给它前面的程序，面板跟着失去焦点，看上去就像被关掉了。
pub fn refocus() -> bool {
    let hwnd = window();
    if hwnd.is_null() || unsafe { IsWindowVisible(hwnd) } == 0 {
        return false;
    }
    // SAFETY: hwnd 是已绑定的面板窗口。
    unsafe { bring_to_front(hwnd) };
    is_active()
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
        end_wake(hwnd);
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
/// 临时把本线程的输入队列挂到当前前台线程上即可获准。截屏遮罩也用它。
pub(super) unsafe fn bring_to_front(hwnd: HWND) {
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

/// 把客户区改成 `size`（逻辑像素，按窗口所在显示器的 DPI 换算），左上角不动。
/// 返回大小是否有变化。
unsafe fn resize(hwnd: HWND, size: [f32; 2]) -> bool {
    let dpi = GetDpiForWindow(hwnd);
    let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
    let (width, height) = (
        (size[0] * scale).round() as i32,
        (size[1] * scale).round() as i32,
    );
    let (mut client, mut frame): (RECT, RECT) = (zeroed(), zeroed());
    if GetClientRect(hwnd, &mut client) == 0 || GetWindowRect(hwnd, &mut frame) == 0 {
        return false;
    }
    let (client_width, client_height) = (client.right - client.left, client.bottom - client.top);
    if (client_width - width).abs() <= 1 && (client_height - height).abs() <= 1 {
        return false;
    }
    // 窗口可能带不可见的边框，按窗口与客户区的差值补上。
    let extra_width = (frame.right - frame.left) - client_width;
    let extra_height = (frame.bottom - frame.top) - client_height;
    SetWindowPos(
        hwnd,
        null_mut(),
        0,
        0,
        width + extra_width,
        height + extra_height,
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
    true
}

/// 按锚点摆放窗口，并限制在所在显示器的工作区内（不挡任务栏）。
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
        Anchor::Center => {
            let work = work_area_at(cursor);
            let (work_width, work_height) = (work.right - work.left, work.bottom - work.top);
            (
                work.left + (work_width - width) / 2,
                work.top + (work_height - height) / 2,
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

/// 鼠标的屏幕坐标（物理像素）。
pub(super) unsafe fn cursor_position() -> POINT {
    let mut point = POINT { x: 0, y: 0 };
    GetCursorPos(&mut point);
    point
}

/// 点所在显示器的工作区（扣除任务栏）。
pub(super) unsafe fn work_area_at(point: POINT) -> RECT {
    let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(monitor, &mut info);
    info.rcWork
}

/// 搜索页的窗口行为（§4.2：由宿主平台层实现）。
///
/// Windows 上没有需要额外调整的层级或空格处理；宿主也没有全局绑定空格键。
/// 保留这个入口，方便以后接入原生焦点/输入法处理。
pub fn set_search_space_behavior(_searching: bool) {}
