//! 无边框窗口的缩放边框（Windows）。
//!
//! eframe 用 `with_decorations(false)` 建窗口时，winit 会把客户区扩到整个窗口矩形
//! （只留下顶部一个物理像素的非客户区，那是它铺投影时留下的偏移），系统因此没有可以
//! 拖动的边框：四边和四个角都不出现缩放鼠标，只有顶部那一个像素能拖，看起来就像
//! “只有上面两个角能用”。macOS 的 `NSWindow` 不受影响，没有标题栏也能从边缘缩放，
//! 所以那边是正常的。
//!
//! 这里用 `SetWindowSubclass` 接住 `WM_NCHITTEST`，在窗口四周返回 `HTLEFT`、
//! `HTTOPRIGHT` 这些命中码：系统据此把鼠标换成缩放箭头，并由系统自己完成拖动缩放，
//! 和带边框的窗口一致。宽度按逻辑像素算，再按窗口 DPI 换算成物理像素。
//!
//! 注意这条边框会占掉客户区最外面几个像素：面板右侧的滚动条平时只有两个像素宽，
//! 但 egui 的滚动条交互矩形按展开后的宽度算（`bar_width`，默认 10），所以剩下的一段
//! 仍然够鼠标移上去把它展开。

use std::mem::zeroed;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT,
    HTTOPRIGHT, IsIconic, IsZoomed,
};

/// `WM_NCHITTEST`。
const WM_NCHITTEST: u32 = 0x0084;

/// 缩放边框的宽度（逻辑像素）。与带标题栏的窗口大致相当。
const BORDER: f64 = 5.0;

/// 只用来区分同一个窗口上的这一层子类化，取值本身没有含义。
const SUBCLASS_ID: usize = 0x5253_5a42;

/// 给窗口四周加回缩放边框。窗口句柄没被销毁之前只需调用一次。
pub fn add_borders(hwnd: HWND) {
    // SAFETY: hwnd 来自 winit 创建的窗口；子类化回调只在创建它的线程的消息循环里被调用。
    unsafe {
        if SetWindowSubclass(hwnd, Some(proc), SUBCLASS_ID, 0) == 0 {
            tracing::warn!("给窗口加缩放边框失败");
        }
    }
}

unsafe extern "system" fn proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_NCHITTEST {
        // SAFETY: 参数由系统传入，hwnd 是本窗口。
        if let Some(hit) = unsafe { hit_test(hwnd, lparam) } {
            return hit as LRESULT;
        }
    }
    // SAFETY: 其余消息交给原来的窗口过程（winit 的）。
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// 靠近窗口边缘时返回对应的命中码；不在边缘、或者窗口最大化 / 最小化时返回 `None`。
unsafe fn hit_test(hwnd: HWND, lparam: LPARAM) -> Option<u32> {
    // 最大化时客户区铺满屏幕、最小化时没有窗口矩形，都不该缩放。
    if unsafe { IsZoomed(hwnd) } != 0 || unsafe { IsIconic(hwnd) } != 0 {
        return None;
    }
    let mut frame: RECT = unsafe { zeroed() };
    // SAFETY: frame 是栈上的结构体。
    if unsafe { GetWindowRect(hwnd, &mut frame) } == 0 {
        return None;
    }
    // lParam 的低 16 位是 x、高 16 位是 y（屏幕坐标，有符号）。
    let x = (lparam as u16 as i16) as i32;
    let y = ((lparam >> 16) as u16 as i16) as i32;
    let border = (BORDER * f64::from(unsafe { GetDpiForWindow(hwnd) }) / 96.0).round() as i32;
    let left = x - frame.left < border;
    let right = frame.right - x < border;
    let top = y - frame.top < border;
    let bottom = frame.bottom - y < border;
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, _, _, _) => HTLEFT,
        (_, true, _, _) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (_, _, _, true) => HTBOTTOM,
        _ => return None,
    })
}
