//! 截屏、显示器与窗口位置（Windows）。
//!
//! - 截屏：用 GDI 从屏幕 DC `BitBlt` 到 32 位 DIB（带 `CAPTUREBLT`，分层窗口也截得到）；
//! - 滚动截图：同样 `BitBlt` 选区，但不带 `CAPTUREBLT`（每秒截几十次，它会让鼠标指针跟着闪烁）；
//!   遮罩挖掉了选区并且不进入截屏（见 `floating::begin_scroll`）。
//! - 窗口：`EnumWindows` 按 Z 序（从上到下）枚举顶层窗口，取 DWM 的 `DWMWA_EXTENDED_FRAME_BOUNDS`
//!   （不含 Windows 10/11 窗口四周不可见的缩放边框），跳过隐藏、最小化、被 DWM 隐藏（cloaked，
//!   例如其他虚拟桌面上的窗口）、鼠标穿透的窗口和本程序自己的窗口。
//!
//! 坐标都是物理像素（winit 已把进程设为按显示器感知 DPI）。截屏不需要额外的权限。

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use anyhow::{bail, Result};
use egui::{pos2, vec2, Rect};
use image::RgbaImage;
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
    MonitorFromPoint, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
    DIB_RGB_COLORS, MONITORINFO, MONITOR_DEFAULTTONEAREST, SRCCOPY,
};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetWindowLongW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    GWL_EXSTYLE, MB_OK, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};

use crate::snip::{Display, ScreenWindow};

/// Windows 截屏不需要授权。
pub fn capture_allowed() -> bool {
    true
}

/// Windows 截屏不需要授权。
pub fn request_capture_access() -> bool {
    true
}

/// Windows 没有对应的设置页。
pub fn open_capture_settings() {}

/// 提示音（例如剪贴板里没有图片可贴）。
pub fn beep() {
    // SAFETY: 只播放系统提示音。
    unsafe { MessageBeep(MB_OK) };
}

/// 鼠标位置（物理像素）与它所在的显示器。
pub fn cursor() -> Option<([f64; 2], Display)> {
    // SAFETY: 输出参数都是本地变量；`cbSize` 按要求设置。
    unsafe {
        let mut point = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut point) == 0 {
            return None;
        }
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        let (mut dpi_x, mut dpi_y) = (96, 96);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let scale = dpi_x.max(96) as f32 / 96.0;
        let rc = info.rcMonitor;
        let (width, height) = (rc.right - rc.left, rc.bottom - rc.top);
        let display = Display {
            id: 0,
            frame: [
                f64::from(rc.left),
                f64::from(rc.top),
                f64::from(width),
                f64::from(height),
            ],
            // winit 的逻辑坐标是物理像素除以窗口所在显示器的缩放比例；多个显示器缩放不同时只是近似，
            // 截屏遮罩出现后由 `floating::begin_overlay` 按物理像素摆正。
            logical: Rect::from_min_size(
                pos2(rc.left as f32 / scale, rc.top as f32 / scale),
                vec2(width as f32 / scale, height as f32 / scale),
            ),
            scale,
        };
        Some(([f64::from(point.x), f64::from(point.y)], display))
    }
}

/// 截取桌面上的一块区域（`area` 是全局坐标 `[x, y, 宽, 高]`，物理像素），返回 RGBA 图像（滚动截图用）。
/// 遮罩在进入滚动截图时已经设为不进入截屏（`floating::begin_scroll`），截到的就是它下面的内容，用不到 `_below`。
pub fn capture_area(area: [f64; 4], _below: Option<u32>) -> Result<RgbaImage> {
    let [x, y, width, height] = area.map(|v| v.round() as i32);
    blt([x, y, width, height], SRCCOPY)
}

/// 截取整个显示器，返回物理像素的 RGBA 图像。
pub fn capture(display: &Display) -> Result<RgbaImage> {
    blt(display.frame.map(|v| v as i32), SRCCOPY | CAPTUREBLT)
}

/// 从屏幕 DC 把 `[x, y, 宽, 高]`（物理像素）`BitBlt` 出来，转成 RGBA。
fn blt(area: [i32; 4], rop: u32) -> Result<RgbaImage> {
    let [x, y, width, height] = area;
    if width <= 0 || height <= 0 {
        bail!("截取范围无效：{width}×{height}");
    }
    // SAFETY: 创建的 DC、位图都在返回前释放；DIB 的像素缓冲区由系统分配，大小为 width × height × 4。
    unsafe {
        let screen = GetDC(null_mut());
        let memory = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // 负数表示自上而下存放，第一行就是屏幕最上面一行。
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..zeroed()
        };
        let mut bits: *mut c_void = null_mut();
        let bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let mut result = None;
        if !bitmap.is_null() && !bits.is_null() {
            let old = SelectObject(memory, bitmap);
            if BitBlt(memory, 0, 0, width, height, screen, x, y, rop) != 0 {
                let len = width as usize * height as usize * 4;
                let bgra = std::slice::from_raw_parts(bits.cast::<u8>(), len);
                let mut rgba = Vec::with_capacity(len);
                for [b, g, r, _] in bgra.as_chunks::<4>().0 {
                    rgba.extend_from_slice(&[*r, *g, *b, 255]);
                }
                result = RgbaImage::from_raw(width as u32, height as u32, rgba);
            }
            SelectObject(memory, old);
            DeleteObject(bitmap);
        }
        DeleteDC(memory);
        ReleaseDC(null_mut(), screen);
        match result {
            Some(image) => Ok(image),
            None => bail!("截屏失败（BitBlt）"),
        }
    }
}

/// 屏幕上其他程序的窗口，从上到下排列（物理像素）。
pub fn windows() -> Vec<ScreenWindow> {
    let mut out: Vec<ScreenWindow> = Vec::new();
    // SAFETY: 回调只在 EnumWindows 返回前被调用，lparam 指向本函数里的 out。
    unsafe {
        EnumWindows(Some(collect), &mut out as *mut Vec<ScreenWindow> as LPARAM);
    }
    out
}

/// `EnumWindows` 的回调：符合条件的窗口加入列表，继续枚举。
unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam as *mut Vec<ScreenWindow>);
    if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
        return 1;
    }
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == GetCurrentProcessId() {
        return 1;
    }
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex_style & WS_EX_TRANSPARENT != 0 {
        return 1;
    }
    let mut cloaked: u32 = 0;
    let cloaked_ok = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED as u32,
        (&mut cloaked as *mut u32).cast(),
        size_of::<u32>() as u32,
    ) == 0;
    if cloaked_ok && cloaked != 0 {
        return 1;
    }
    let mut rect: RECT = zeroed();
    let got = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS as u32,
        (&mut rect as *mut RECT).cast(),
        size_of::<RECT>() as u32,
    ) == 0;
    if !got {
        return 1;
    }
    out.push(ScreenWindow {
        frame: [
            f64::from(rect.left),
            f64::from(rect.top),
            f64::from(rect.right - rect.left),
            f64::from(rect.bottom - rect.top),
        ],
        floating: ex_style & WS_EX_TOPMOST != 0,
    });
    1
}
