//! 截屏遮罩与贴图窗口的原生设置（Windows）。
//!
//! 两种窗口都由 egui 的多视口创建，这里按窗口标题在本线程的窗口里找到句柄再调整：
//! - 截屏遮罩：按物理像素铺满鼠标所在的显示器并置顶（winit 按逻辑坐标创建窗口，
//!   多个显示器缩放比例不同时位置不准），设为前台窗口接收按键；结束后把前台还给之前的窗口。
//! - 贴图：winit 的置顶窗口即可；点击贴图会让它成为前台窗口，与 Snipaste 一致。
//! - 滚动截图：遮罩是不透明窗口，用 `SetWindowRgn` 只保留边框、提示、预览和工具栏这几块，
//!   选区外的调暗交给另一个半透明的 GDI 窗口（`scroll_mask`），它挖掉选区：选区内外看到的都是实时的屏幕，
//!   选区里的鼠标、滚轮直接落到下面的窗口（不用 winit 的鼠标穿透：它会加上 `WS_EX_LAYERED`，见下）；
//!   再用 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` 让遮罩不进入截屏，
//!   放进选区里的工具栏、预览（选区几乎占满屏幕时）也不会被截进去。
//!
//! 贴图的不透明度在 Windows 上没有实现：分层窗口（`WS_EX_LAYERED`）与 wgpu 使用的 DXGI 翻转模型交换链不兼容。

use std::cell::Cell;
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_TRANSITIONS_FORCEDISABLED,
};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, SetWindowRgn,
    HRGN, RGN_OR,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowRect,
    GetWindowTextW, IsWindow, SetForegroundWindow, SetWindowDisplayAffinity, SetWindowPos,
    ShowWindow, ShowWindowAsync, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_HIDE, WDA_EXCLUDEFROMCAPTURE,
};

use super::{panel, scroll_mask, wide, workspace};

/// 贴图窗口是否支持调整不透明度。
pub const OPACITY_SUPPORTED: bool = false;

thread_local! {
    /// 截屏前的前台窗口，截屏结束后把前台还给它（只在主线程访问）。
    static PREVIOUS: Cell<HWND> = const { Cell::new(null_mut()) };
    /// 正在拖动的贴图：窗口与按住的那一点相对窗口矩形左上角的位置（物理像素，见 [`start_drag`]）。
    static DRAG: Cell<Option<(HWND, POINT)>> = const { Cell::new(None) };
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

/// 遮罩窗口创建时是否隐藏（见 [`show_overlay`]）。
///
/// Windows 上遮罩是不透明窗口（wgpu 的透明窗口不稳定）：一创建就显示的话，第一帧画出来之前是一块白色，
/// 而且 winit 按创建时的缩放换算逻辑尺寸，窗口只有屏幕的一部分大；`begin_overlay` 铺满屏幕后，
/// 第一帧还按旧尺寸画，其余部分是黑的（录屏里截屏开始时闪一下）。所以先隐藏着创建，
/// 由主界面摆好位置、用 DWM 遮住（cloak）后再显示，按正确尺寸画出一帧后才去掉遮挡（[`reveal_overlay`]）。
pub const OVERLAY_STARTS_HIDDEN: bool = true;

/// 主界面调用：把隐藏着创建的遮罩摆到 `frame`（铺满显示器）、遮住后显示出来，eframe 随后运行遮罩的界面。
/// 返回是否找到了窗口（窗口还没创建时下一帧再试）。
pub fn show_overlay(title: &str, frame: [f64; 4]) -> bool {
    let hwnd = window(title);
    if hwnd.is_null() {
        return false;
    }
    disable_transitions(hwnd);
    set_cloaked(hwnd, true);
    // SAFETY: hwnd 是本线程的窗口。
    unsafe { place_overlay(hwnd, frame, SWP_SHOWWINDOW | SWP_NOACTIVATE) };
    true
}

/// 遮罩按正确尺寸画出第一帧后调用：去掉 [`show_overlay`] 加的遮挡。
pub fn reveal_overlay(title: &str) {
    let hwnd = window(title);
    if !hwnd.is_null() {
        set_cloaked(hwnd, false);
    }
}

/// 用 DWM 遮住 / 露出窗口：遮住时窗口照常“可见”、照常绘制，只是不显示在屏幕上。
fn set_cloaked(hwnd: HWND, cloaked: bool) {
    let value: BOOL = cloaked.into();
    // SAFETY: hwnd 是本线程的顶层窗口；属性值是 4 字节的 BOOL。
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK as u32,
            (&value as *const BOOL).cast(),
            size_of::<BOOL>() as u32,
        );
    }
}

/// 按物理像素把遮罩的客户区摆到 `frame`，并置顶。
///
/// winit 的无边框窗口保留了部分边框样式，客户区不一定与窗口矩形重合（录屏里遮罩收起时整个画面错开了约 1 像素）：
/// 先摆好窗口，再按客户区与目标的偏差修正一次。
///
/// SAFETY: hwnd 是本线程的窗口。
unsafe fn place_overlay(hwnd: HWND, frame: [f64; 4], flags: u32) {
    let [x, y, width, height] = frame.map(|v| v as i32);
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, flags);
    let mut window: RECT = zeroed();
    let mut client: RECT = zeroed();
    let mut origin = POINT { x: 0, y: 0 };
    if GetWindowRect(hwnd, &mut window) == 0
        || GetClientRect(hwnd, &mut client) == 0
        || ClientToScreen(hwnd, &mut origin) == 0
    {
        return;
    }
    let (left, top) = (origin.x - window.left, origin.y - window.top);
    let extra_width = (window.right - window.left) - client.right;
    let extra_height = (window.bottom - window.top) - client.bottom;
    if left != 0 || top != 0 || extra_width != 0 || extra_height != 0 {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x - left,
            y - top,
            width + extra_width,
            height + extra_height,
            flags | SWP_NOACTIVATE,
        );
    }
}

/// 关掉 DWM 给窗口显示、隐藏加的淡入淡出（与 macOS 上关掉窗口动画一样）：遮罩画的是整屏的静止截图，
/// 收起时淡出看起来整屏闪一下；贴图与 Snipaste 一样立即出现、消失。
pub(super) fn disable_transitions(hwnd: HWND) {
    let disabled: BOOL = 1;
    // SAFETY: hwnd 是本线程的窗口；属性值是 4 字节的 BOOL。
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            (&disabled as *const BOOL).cast(),
            size_of::<BOOL>() as u32,
        );
    }
}

/// 截屏遮罩出现后调用：按物理像素铺满显示器（`frame`）、置顶并设为前台。
/// 返回是否找到了窗口（窗口还没创建时下一帧再试）。
pub fn begin_overlay(title: &str, frame: [f64; 4]) -> bool {
    let hwnd = window(title);
    if hwnd.is_null() {
        return false;
    }
    disable_transitions(hwnd);
    // 显示之前一定先遮住：eframe 创建视口的那一帧就会运行遮罩的界面，而主界面在这一帧已经跑过了，
    // 没有机会调用 [`show_overlay`]。这时窗口还从没被遮住过，下面的 `SWP_SHOWWINDOW` 一显示，
    // 屏幕上就是还没画过的白底，接着是被拉伸的旧尺寸画面，然后才是正确的截图——闪两三帧。
    // 已经被 `show_overlay` 遮住时再遮一次没有副作用。
    set_cloaked(hwnd, true);
    // SAFETY: hwnd 是本线程的窗口。
    unsafe {
        let foreground = GetForegroundWindow();
        if workspace::is_other_app(foreground) {
            PREVIOUS.with(|cell| cell.set(foreground));
        }
        place_overlay(hwnd, frame, SWP_SHOWWINDOW);
        panel::bring_to_front(hwnd);
    }
    OVERLAY.store(hwnd as isize, Ordering::Relaxed);
    true
}

/// 截屏遮罩是不透明窗口（wgpu 的透明窗口不稳定）：滚动截图时只保留几块（[`set_scroll_regions`]），
/// 选区外的调暗交给另一个窗口。
pub const OVERLAY_TRANSPARENT: bool = false;

/// 进入滚动截图时调用：让遮罩不进入截屏（`WDA_EXCLUDEFROMCAPTURE`，Windows 10 2004 起支持），
/// 截取选区时截到的是遮罩下面的内容；创建选区外的半透明遮罩，颜色是 `mask`（预乘的，与界面上画的一样）。
/// 返回遮罩的窗口编号（找不到窗口时为空）；`screen::capture_area` 用不到它，遮罩已经不会被截到。
pub fn begin_scroll(title: &str, mask: egui::Color32) -> Option<u32> {
    let hwnd = window(title);
    if hwnd.is_null() {
        return None;
    }
    // SAFETY: hwnd 是本线程的顶层窗口。
    if unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) } == 0 {
        // 旧版本的 Windows：选区里一般没有遮罩的内容，截不到它；只是选区几乎占满屏幕、
        // 工具栏或预览放进了选区里时会被截进去。
        tracing::warn!("SetWindowDisplayAffinity 失败，滚动截图可能截到遮罩");
    }
    let [r, g, b, a] = mask.to_srgba_unmultiplied();
    scroll_mask::create(hwnd, [r, g, b], a);
    // 64 位 Windows 上窗口句柄只用低 32 位（与 32 位进程互通）。
    Some(hwnd as usize as u32)
}

/// 滚动截图时遮罩的样子：截屏遮罩只显示 `shown` 这几块（矩形与圆角半径：边框、提示、预览、工具栏），
/// 其余部分从窗口里去掉；选区外的半透明遮罩挖掉 `hole`（选区连同它与边框之间的空隙）。
/// 坐标都是相对遮罩客户区的物理像素（与截图的像素一致，遮罩的客户区正好铺满显示器）。
pub fn set_scroll_regions(title: &str, hole: egui::Rect, shown: &[(egui::Rect, f32)]) {
    let hwnd = window(title);
    if hwnd.is_null() {
        return;
    }
    let pixels = |rect: egui::Rect| {
        [
            rect.min.x.floor() as i32,
            rect.min.y.floor() as i32,
            rect.max.x.ceil() as i32,
            rect.max.y.ceil() as i32,
        ]
    };
    scroll_mask::set_hole(hwnd, pixels(hole));
    // SAFETY: hwnd 是本线程的窗口；区域对象除了交给 SetWindowRgn 的那个（之后归系统所有）都在这里释放。
    unsafe {
        let Some((dx, dy)) = client_offset(hwnd) else {
            return;
        };
        let region: HRGN = CreateRectRgn(0, 0, 0, 0);
        for (rect, radius) in shown {
            let [left, top, right, bottom] = pixels(*rect);
            let part = if *radius >= 1.0 {
                // 圆角矩形的区域比参数小一格（不含右下边界），所以右下各加一。
                let diameter = (radius * 2.0).round() as i32;
                CreateRoundRectRgn(
                    left + dx,
                    top + dy,
                    right + dx + 1,
                    bottom + dy + 1,
                    diameter,
                    diameter,
                )
            } else {
                CreateRectRgn(left + dx, top + dy, right + dx, bottom + dy)
            };
            CombineRgn(region, region, part, RGN_OR);
            DeleteObject(part);
        }
        if SetWindowRgn(hwnd, region, 1) == 0 {
            DeleteObject(region);
            tracing::warn!("SetWindowRgn 失败，滚动截图时看不到下面的窗口");
        }
    }
}

/// 结束滚动截图：去掉选区外的遮罩，截屏遮罩什么都不显示（随后由主界面收起），看到的就是真实的屏幕。
pub fn end_scroll(title: &str) {
    scroll_mask::close();
    let hwnd = window(title);
    if !hwnd.is_null() {
        // SAFETY: hwnd 是本线程的窗口；空区域交给 SetWindowRgn 后归系统所有，失败时在这里释放。
        unsafe {
            let empty = CreateRectRgn(0, 0, 0, 0);
            if SetWindowRgn(hwnd, empty, 1) == 0 {
                DeleteObject(empty);
            }
        }
    }
}

/// 窗口区域的坐标相对窗口矩形的左上角，不是客户区（两者不一定重合，见 `place_overlay`）：
/// 返回客户区左上角相对窗口矩形左上角的偏移。
///
/// SAFETY: hwnd 是有效的窗口。
unsafe fn client_offset(hwnd: HWND) -> Option<(i32, i32)> {
    let mut window: RECT = zeroed();
    let mut origin = POINT { x: 0, y: 0 };
    if GetWindowRect(hwnd, &mut window) == 0 || ClientToScreen(hwnd, &mut origin) == 0 {
        return None;
    }
    Some((origin.x - window.left, origin.y - window.top))
}

/// 收起截屏遮罩（egui 随后销毁窗口）。
pub fn hide_overlay(title: &str) {
    scroll_mask::close();
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
    scroll_mask::hide_async();
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

/// 贴图窗口出现后调用：关掉显示、隐藏的过渡动画（置顶、不在任务栏显示都由 winit 完成）。
/// 返回是否找到了窗口。
pub fn configure_pin(title: &str) -> bool {
    let hwnd = window(title);
    if hwnd.is_null() {
        return false;
    }
    disable_transitions(hwnd);
    true
}

/// 贴图窗口销毁前不需要处理（macOS 上要把换过的类换回去）。
pub fn release_pin(_title: &str) {}

/// 不支持（见模块说明）。
pub fn set_opacity(_title: &str, _opacity: f32) {}

/// 无边框窗口在 Windows 上没有系统阴影，不处理。
pub fn set_shadow(_title: &str, _shadow: bool) {}

/// 按住鼠标拖动贴图窗口：之后每帧由 [`drag_moved`] 让窗口跟着鼠标移动，松开时 [`end_drag`]。
/// `grab` 是按下鼠标的位置（相对窗口客户区左上角，点）：开始拖动时鼠标已经移动了一小段，
/// 按这个位置算，按住的那一点始终在鼠标下面。
///
/// 不用 winit 的 `drag_window`：它走系统的窗口移动，系统会把窗口顶部限制在屏幕里，
/// 比屏幕还高的长截图贴图就拖不上去了。返回 false 表示没找到窗口，由调用方交给 winit。
pub fn start_drag(title: &str, grab: egui::Vec2) -> bool {
    let hwnd = window(title);
    if hwnd.is_null() {
        return false;
    }
    // SAFETY: hwnd 是本线程的窗口。
    let Some((dx, dy)) = (unsafe { client_offset(hwnd) }) else {
        return false;
    };
    // SAFETY: 同上。
    let scale = f64::from(unsafe { GetDpiForWindow(hwnd) }.max(96)) / 96.0;
    // 鼠标相对窗口矩形左上角的位置（物理像素）。
    let offset = POINT {
        x: (f64::from(grab.x) * scale).round() as i32 + dx,
        y: (f64::from(grab.y) * scale).round() as i32 + dy,
    };
    DRAG.with(|drag| drag.set(Some((hwnd, offset))));
    drag_moved();
    true
}

/// 拖动中：移动窗口，让按住的那一点回到鼠标下面。
pub fn drag_moved() {
    let Some((hwnd, offset)) = DRAG.with(Cell::get) else {
        return;
    };
    // SAFETY: 输出参数是本地变量；窗口可能已经关闭，SetWindowPos 对无效句柄只返回失败。
    unsafe {
        let mut cursor = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut cursor) == 0 {
            return;
        }
        SetWindowPos(
            hwnd,
            null_mut(),
            cursor.x - offset.x,
            cursor.y - offset.y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 松开鼠标，结束拖动。
pub fn end_drag() {
    DRAG.with(|drag| drag.set(None));
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
///
/// `restore_focus` 与 macOS 那边保持一致的签名：Windows 的对话框关掉后焦点自己回到原来的窗口，
/// 这里不需要额外处理（调用方要还焦点的话另外调 [`restore_focus`]）。
pub fn save_dialog(
    file_name: &str,
    directory: Option<&Path>,
    _restore_focus: bool,
) -> Option<PathBuf> {
    let mut buffer = [0u16; 1024];
    let name: Vec<u16> = file_name.encode_utf16().take(buffer.len() - 1).collect();
    buffer[..name.len()].copy_from_slice(&name);
    // 过滤器是以两个 NUL 结尾的“说明\0模式\0”序列，`wide` 会补上最后一个 NUL。
    let filter = wide("PNG 图片\0*.png\0");
    let extension = wide("png");
    let directory = directory.map(|dir| wide(&dir.to_string_lossy()));
    // 对话框打开期间面板会失去焦点，界面据此不要把它自动收起（见 [`dialog_open`]）。
    DIALOG_OPEN.store(true, Ordering::Relaxed);
    // SAFETY: 结构体中的指针都指向本函数里的缓冲区，对话框返回前一直有效。
    let chosen = unsafe {
        let mut dialog: OPENFILENAMEW = zeroed();
        dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrFile = buffer.as_mut_ptr();
        dialog.nMaxFile = buffer.len() as u32;
        dialog.lpstrDefExt = extension.as_ptr();
        dialog.lpstrInitialDir = directory.as_ref().map_or(null(), |dir| dir.as_ptr());
        dialog.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        GetSaveFileNameW(&mut dialog) != 0
    };
    DIALOG_OPEN.store(false, Ordering::Relaxed);
    if !chosen {
        return None;
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(crate::snip::ensure_png(PathBuf::from(
        String::from_utf16_lossy(&buffer[..len]),
    )))
}

/// 保存对话框是否正在显示（见 [`save_dialog`]）。
static DIALOG_OPEN: AtomicBool = AtomicBool::new(false);

/// 是否正在显示本应用弹出的系统对话框。对话框打开期间面板失去焦点是正常的，
/// 界面层据此不要自动收起面板（见 `HubApp::handle_focus`）。
pub fn dialog_open() -> bool {
    DIALOG_OPEN.load(Ordering::Relaxed)
}

// `windows-sys` 0.61 没有导出 `SHBrowseForFolderW`（旧 API，被 IFileDialog 取代），
// 这里手动声明这两个 shell32 函数；结构体 `BROWSEINFOW` 仍在 `Win32_UI_Shell` 里。
#[link(name = "shell32")]
extern "system" {
    fn SHBrowseForFolderW(info: *mut windows_sys::Win32::UI::Shell::BROWSEINFOW)
        -> *mut std::ffi::c_void;
    fn SHGetPathFromIDListW(pidl: *const std::ffi::c_void, path: *mut u16) -> i32;
}

/// 选择文件夹（文件搜索模块的索引根）。取消返回 `Ok(None)`。
///
/// 用 `SHBrowseForFolderW`（不需要 COM 接口对象）；`initial` 暂不支持——
/// 指定初始目录需要 BFFM_SETSELECTION 回调，v1 从默认位置开始。
pub fn pick_folder(_initial: Option<&Path>) -> Result<Option<PathBuf>, String> {
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows_sys::Win32::UI::Shell::{
        BIF_EDITBOX, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
    };
    // SAFETY: 结构体里的指针都指向本函数内的缓冲区；对话框返回后立即释放 pidl。
    unsafe {
        CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
    }
    let title: Vec<u16> = "选择要建立索引的文件夹"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut display = [0u16; 260];
    DIALOG_OPEN.store(true, Ordering::Relaxed);
    // SAFETY: 同上；pidl 由 shell32 分配，下面立即释放。
    let pidl = unsafe {
        let mut info: BROWSEINFOW = zeroed();
        info.pszDisplayName = display.as_mut_ptr();
        info.lpszTitle = title.as_ptr();
        info.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE | BIF_EDITBOX;
        SHBrowseForFolderW(&mut info)
    };
    DIALOG_OPEN.store(false, Ordering::Relaxed);
    if pidl.is_null() {
        unsafe { CoUninitialize() };
        return Ok(None);
    }
    let mut path = [0u16; 260];
    // SAFETY: pidl 来自上面的 SHBrowseForFolderW。
    let ok = unsafe { SHGetPathFromIDListW(pidl, path.as_mut_ptr()) };
    unsafe {
        CoTaskMemFree(pidl);
        CoUninitialize();
    }
    if ok == 0 {
        return Err("无法读取所选文件夹的路径".into());
    }
    let len = path.iter().position(|&c| c == 0).unwrap_or(path.len());
    Ok(Some(PathBuf::from(String::from_utf16_lossy(&path[..len]))))
}

/// 搜索模块的原生模态开始：标记"对话框已打开"，面板不会因为失焦被收起。
pub fn begin_system_modal() -> bool {
    DIALOG_OPEN.store(true, Ordering::Relaxed);
    true
}

/// 原生模态结束。
pub fn end_system_modal(_restore_focus: bool) {
    DIALOG_OPEN.store(false, Ordering::Relaxed);
}
