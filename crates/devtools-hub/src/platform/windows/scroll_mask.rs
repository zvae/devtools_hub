//! 滚动截图时选区外的半透明遮罩（Windows）。
//!
//! 截屏遮罩是 wgpu 画的不透明窗口，不能半透明地盖在实时的屏幕上（分层窗口与 DXGI 翻转模型交换链不兼容，
//! 见 `floating`）。滚动截图时选区外的调暗改由这里的一个 GDI 窗口负责：纯色背景、`LWA_ALPHA` 的分层窗口，
//! 铺满显示器、挖掉选区，摆在截屏遮罩的正下方；截屏遮罩自己只保留边框、提示、预览和工具栏这几块
//! （`floating::set_scroll_regions`）。于是选区外看到的也是实时的屏幕，与 macOS 上的透明遮罩一样。
//!
//! 这个窗口接住选区外的鼠标（与 macOS 上选区外不穿透一致，点不到下面的程序），但不激活：
//! 键盘焦点留在截屏遮罩上，esc、Enter 照常有效。它也不进入截屏（`WDA_EXCLUDEFROMCAPTURE`）。

use std::mem::zeroed;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Once;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, CombineRgn, CreateRectRgn, CreateSolidBrush, DeleteObject, FillRect,
    SetWindowRgn, HBRUSH, HDC, RGN_DIFF,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW, LoadCursorW,
    RegisterClassW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowLongPtrW,
    SetWindowPos, ShowWindowAsync, GWLP_USERDATA, IDC_ARROW, LWA_ALPHA, MA_NOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, WDA_EXCLUDEFROMCAPTURE,
    WM_ERASEBKGND, WM_MOUSEACTIVATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use super::floating::disable_transitions;
use super::wide;

/// 窗口类名。
const CLASS: &str = "DevToolsHubScrollMask";

/// 正在显示的遮罩窗口（只在主线程创建、销毁；看门狗在其他线程上用它收起，见 [`hide_async`]）。
static MASK: AtomicIsize = AtomicIsize::new(0);

/// 在截屏遮罩 `overlay` 的正下方创建选区外的遮罩（先不显示，[`set_hole`] 挖好选区后才显示）。
/// `color` 是不预乘的 sRGB 颜色，`alpha` 是不透明度。已经有遮罩时什么也不做。
pub(super) fn create(overlay: HWND, color: [u8; 3], alpha: u8) {
    if MASK.load(Ordering::Relaxed) != 0 {
        return;
    }
    static REGISTER: Once = Once::new();
    let class = wide(CLASS);
    // SAFETY: 注册窗口类、创建窗口；所有指针参数都指向本函数里的缓冲区。
    unsafe {
        let instance = GetModuleHandleW(null());
        REGISTER.call_once(|| {
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(window_proc);
            wc.hInstance = instance;
            wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
            wc.lpszClassName = class.as_ptr();
            if RegisterClassW(&wc) == 0 {
                tracing::warn!("注册滚动截图遮罩的窗口类失败");
            }
        });
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            tracing::warn!("创建滚动截图遮罩失败，选区外不会变暗");
            return;
        }
        let [r, g, b] = color.map(u32::from);
        let brush = CreateSolidBrush(r | (g << 8) | (b << 16));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, brush as isize);
        SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        disable_transitions(hwnd);
        // 铺满截屏遮罩的客户区（即整个显示器），先不显示。
        let mut client: RECT = zeroed();
        let mut origin = POINT { x: 0, y: 0 };
        if GetClientRect(overlay, &mut client) != 0 && ClientToScreen(overlay, &mut origin) != 0 {
            SetWindowPos(
                hwnd,
                overlay,
                origin.x,
                origin.y,
                client.right,
                client.bottom,
                SWP_NOACTIVATE,
            );
        }
        MASK.store(hwnd as isize, Ordering::Relaxed);
    }
}

/// 挖掉选区 `hole`（相对显示器左上角的物理像素，含右下边界外的一格）并显示在截屏遮罩 `overlay` 的正下方。
pub(super) fn set_hole(overlay: HWND, hole: [i32; 4]) {
    let hwnd = MASK.load(Ordering::Relaxed) as HWND;
    if hwnd.is_null() {
        return;
    }
    let [left, top, right, bottom] = hole;
    // SAFETY: hwnd 是本线程创建的窗口；区域对象交给 SetWindowRgn 后归系统所有，失败时在这里释放。
    unsafe {
        let mut client: RECT = zeroed();
        GetClientRect(hwnd, &mut client);
        let region = CreateRectRgn(0, 0, client.right, client.bottom);
        let cut = CreateRectRgn(left, top, right, bottom);
        CombineRgn(region, region, cut, RGN_DIFF);
        DeleteObject(cut);
        if SetWindowRgn(hwnd, region, 1) == 0 {
            DeleteObject(region);
        }
        SetWindowPos(
            hwnd,
            overlay,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE,
        );
    }
}

/// 销毁遮罩（主线程）。
pub(super) fn close() {
    let hwnd = MASK.swap(0, Ordering::Relaxed) as HWND;
    if hwnd.is_null() {
        return;
    }
    // SAFETY: hwnd 是本线程创建的窗口；画刷在窗口销毁后释放。
    unsafe {
        let brush = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as HBRUSH;
        DestroyWindow(hwnd);
        if !brush.is_null() {
            DeleteObject(brush);
        }
    }
}

/// 截屏遮罩卡住时由看门狗调用（任意线程）：收起遮罩，不等主线程处理。
pub(super) fn hide_async() {
    let hwnd = MASK.load(Ordering::Relaxed) as HWND;
    if !hwnd.is_null() {
        // SAFETY: 句柄可能已经失效，ShowWindowAsync 对无效句柄只返回失败。
        unsafe { ShowWindowAsync(hwnd, SW_HIDE) };
    }
}

/// 窗口过程：用纯色画背景；被点击时不激活（焦点留在截屏遮罩上）。
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_ERASEBKGND => {
            let brush = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as HBRUSH;
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            FillRect(wparam as HDC, &rect, brush);
            1
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
