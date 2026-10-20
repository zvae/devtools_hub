//! 截屏、显示器与窗口位置。
//!
//! 与 Snipaste 的 `ScreenGrabber_mac` / `WindowDetector_mac` 是同样的做法：
//! - 截屏：对显示器调用 `CGDisplayCreateImageForRect`，再用 `CGImageCreateCopyWithColorSpace`
//!   标记为 sRGB 后画进位图（只换标记、不做色彩转换，取色得到的就是屏幕上显示的数值）；
//! - 窗口：`CGWindowListCopyWindowInfo` 取屏幕上各窗口的位置（`kCGWindowBounds`）、层级、
//!   不透明度与所属进程，跳过本应用自己的窗口，供截屏时自动框出鼠标下的窗口。
//!
//! - 滚动截图：用 `CGWindowListCreateImage` 只截截屏遮罩下面的窗口（`kCGWindowListOptionOnScreenBelowWindow`），
//!   遮罩上的边框、工具栏、预览不会进入画面（微信用 ScreenCaptureKit 的 `excludingWindows` 达到同样的效果）。
//!
//! 截屏需要“屏幕录制”权限：没有权限时只能截到桌面和菜单栏，其他应用的窗口内容是空的。

use std::ffi::c_void;
use std::ptr;

use anyhow::{bail, Result};
use egui::{pos2, vec2, Rect};
use image::RgbaImage;

use crate::snip::{Display, ScreenWindow};

type CFTypeRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

// 用到的系统函数与常量，名字与 Apple 头文件中的一致。
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGGetDisplaysWithPoint(point: CGPoint, max: u32, displays: *mut u32, count: *mut u32)
        -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGDisplayCopyDisplayMode(display: u32) -> CFTypeRef;
    fn CGDisplayModeGetPixelWidth(mode: CFTypeRef) -> usize;
    fn CGDisplayModeGetWidth(mode: CFTypeRef) -> usize;
    fn CGDisplayModeRelease(mode: CFTypeRef);
    fn CGDisplayCreateImageForRect(display: u32, rect: CGRect) -> CFTypeRef;
    fn CGImageCreateCopyWithColorSpace(image: CFTypeRef, space: CFTypeRef) -> CFTypeRef;
    fn CGImageGetWidth(image: CFTypeRef) -> usize;
    fn CGImageGetHeight(image: CFTypeRef) -> usize;
    fn CGImageRelease(image: CFTypeRef);
    fn CGColorSpaceCreateWithName(name: CFTypeRef) -> CFTypeRef;
    fn CGColorSpaceRelease(space: CFTypeRef);
    static kCGColorSpaceSRGB: CFTypeRef;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CFTypeRef,
        bitmap_info: u32,
    ) -> CFTypeRef;
    fn CGContextDrawImage(context: CFTypeRef, rect: CGRect, image: CFTypeRef);
    fn CGContextRelease(context: CFTypeRef);
    fn CGEventCreate(source: CFTypeRef) -> CFTypeRef;
    fn CGEventGetLocation(event: CFTypeRef) -> CGPoint;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFTypeRef;
    fn CGWindowListCreateImage(
        bounds: CGRect,
        option: u32,
        window: u32,
        image_option: u32,
    ) -> CFTypeRef;
    fn CGRectMakeWithDictionaryRepresentation(dict: CFTypeRef, rect: *mut CGRect) -> bool;
    static kCGWindowBounds: CFTypeRef;
    static kCGWindowLayer: CFTypeRef;
    static kCGWindowOwnerPID: CFTypeRef;
    static kCGWindowAlpha: CFTypeRef;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFArrayGetCount(array: CFTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
    fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFNumberGetValue(number: CFTypeRef, kind: isize, value: *mut c_void) -> bool;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSBeep();
}

// 常量取值见 CGWindow.h、CGImage.h 与 CFNumber.h。
const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_BELOW_WINDOW: u32 = 1 << 2;
const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
/// `kCGWindowImageBestResolution`：按显示器的物理像素截取（Retina 屏为点的 2 倍）。
const K_CG_WINDOW_IMAGE_BEST_RESOLUTION: u32 = 1 << 3;
const K_CG_NULL_WINDOW_ID: u32 = 0;
/// `kCGImageAlphaPremultipliedLast`：按 R、G、B、A 的顺序存放（屏幕像素都不透明，预乘与否没有区别）。
const K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST: u32 = 1;
const K_CF_NUMBER_SINT32_TYPE: isize = 3;
const K_CF_NUMBER_DOUBLE_TYPE: isize = 13;

/// 截屏需要的“屏幕录制”权限是否已经授予。
pub fn capture_allowed() -> bool {
    // SAFETY: 无参数的查询函数（macOS 10.15 起提供）。
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 申请“屏幕录制”权限：第一次调用时系统弹出授权提示，之后只返回当前状态。
pub fn request_capture_access() -> bool {
    // SAFETY: 同上。
    unsafe { CGRequestScreenCaptureAccess() }
}

/// 打开系统设置的“屏幕录制”页面（Snipaste 用的是同一个链接）。
pub fn open_capture_settings() {
    super::workspace::open_url(
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
    );
}

/// 提示音（例如剪贴板里没有图片可贴）。
pub fn beep() {
    // SAFETY: 无参数，只播放系统提示音。
    unsafe { NSBeep() }
}

/// 鼠标位置（全局坐标，点）与它所在的显示器。
pub fn cursor() -> Option<([f64; 2], Display)> {
    // SAFETY: `CGEventCreate(NULL)` 得到一个空事件，读取的位置就是当前鼠标位置；用完释放。
    let point = unsafe {
        let event = CGEventCreate(ptr::null());
        if event.is_null() {
            return None;
        }
        let point = CGEventGetLocation(event);
        CFRelease(event);
        point
    };
    let mut id = 0;
    let mut count = 0;
    // SAFETY: 输出缓冲区容量为 1。
    let found = unsafe { CGGetDisplaysWithPoint(point, 1, &mut id, &mut count) } == 0 && count > 0;
    if !found {
        // SAFETY: 无参数。
        id = unsafe { CGMainDisplayID() };
    }
    Some(([point.x, point.y], display(id)))
}

/// 显示器的位置与缩放比例。
fn display(id: u32) -> Display {
    // SAFETY: id 来自系统；显示模式用完释放。
    let (bounds, scale) = unsafe {
        let bounds = CGDisplayBounds(id);
        let mode = CGDisplayCopyDisplayMode(id);
        let scale = if mode.is_null() {
            1.0
        } else {
            let (pixels, points) = (
                CGDisplayModeGetPixelWidth(mode),
                CGDisplayModeGetWidth(mode),
            );
            CGDisplayModeRelease(mode);
            if points == 0 {
                1.0
            } else {
                pixels as f32 / points as f32
            }
        };
        (bounds, scale)
    };
    let CGRect { origin, size } = bounds;
    Display {
        id,
        frame: [origin.x, origin.y, size.width, size.height],
        // winit 的逻辑坐标与 CG 的全局坐标一致：点，主显示器左上角为原点。
        logical: Rect::from_min_size(
            pos2(origin.x as f32, origin.y as f32),
            vec2(size.width as f32, size.height as f32),
        ),
        scale,
    }
}

/// 截取整个显示器，返回物理像素的 RGBA 图像。
pub fn capture(display: &Display) -> Result<RgbaImage> {
    let [_, _, width, height] = display.frame;
    let local = CGRect {
        origin: CGPoint::default(),
        size: CGSize { width, height },
    };
    // SAFETY: 返回的图像由 `to_rgba` 释放。
    let image = unsafe { CGDisplayCreateImageForRect(display.id, local) };
    if image.is_null() {
        bail!("截屏失败（CGDisplayCreateImageForRect 返回空）");
    }
    // SAFETY: `image` 是刚创建、由我们持有的 CGImage。
    unsafe { to_rgba(image) }
}

/// 截取桌面上的一块区域（`area` 是全局坐标 `[x, y, 宽, 高]`，点），只截窗口 `below`（本应用的截屏遮罩）下面的内容；
/// 返回物理像素的 RGBA 图像。滚动截图时遮罩一直盖在上面，用它排除遮罩自己。
pub fn capture_area(area: [f64; 4], below: Option<u32>) -> Result<RgbaImage> {
    let [x, y, width, height] = area;
    let bounds = CGRect {
        origin: CGPoint { x, y },
        size: CGSize { width, height },
    };
    let (option, window) = match below {
        Some(window) => (K_CG_WINDOW_LIST_OPTION_ON_SCREEN_BELOW_WINDOW, window),
        None => (K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY, K_CG_NULL_WINDOW_ID),
    };
    // SAFETY: 返回的图像由 `to_rgba` 释放。macOS 14 起这个函数标为过时，但仍然可用
    // （微信 4.1、Snipaste 2.10 也在用；macOS 26 上实测可用）。
    let image = unsafe {
        CGWindowListCreateImage(bounds, option, window, K_CG_WINDOW_IMAGE_BEST_RESOLUTION)
    };
    if image.is_null() {
        bail!("截屏失败（CGWindowListCreateImage 返回空）");
    }
    // SAFETY: 同上。
    unsafe { to_rgba(image) }
}

/// 把 CGImage 标记为 sRGB 后画进 RGBA 位图（只换标记、不做色彩转换，取色得到的就是屏幕上显示的数值），
/// 并释放 `image`。
///
/// # Safety
/// `image` 必须是调用方持有的有效 CGImage，调用后不能再使用。
unsafe fn to_rgba(image: CFTypeRef) -> Result<RgbaImage> {
    // SAFETY: 所有 Create / Copy 得到的对象都在返回前释放；位图缓冲区大小与上下文参数一致。
    unsafe {
        let srgb = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        let tagged = CGImageCreateCopyWithColorSpace(image, srgb);
        let source = if tagged.is_null() { image } else { tagged };
        let (w, h) = (CGImageGetWidth(source), CGImageGetHeight(source));
        let mut pixels = vec![0u8; w * h * 4];
        let context = CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            w,
            h,
            8,
            w * 4,
            srgb,
            K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST,
        );
        if !context.is_null() {
            let rect = CGRect {
                origin: CGPoint::default(),
                size: CGSize {
                    width: w as f64,
                    height: h as f64,
                },
            };
            CGContextDrawImage(context, rect, source);
            CGContextRelease(context);
        }
        if !tagged.is_null() {
            CGImageRelease(tagged);
        }
        CGImageRelease(image);
        CGColorSpaceRelease(srgb);
        if context.is_null() {
            bail!("创建位图失败（{w}×{h}）");
        }
        RgbaImage::from_raw(w as u32, h as u32, pixels)
            .ok_or_else(|| anyhow::anyhow!("图像大小不符"))
    }
}

/// 屏幕上其他应用的窗口，从上到下排列（全局坐标，点）。
///
/// 与 Snipaste 一样只看在屏幕上的窗口（`kCGWindowListOptionOnScreenOnly`），跳过完全透明的窗口和本应用的窗口；
/// 另外跳过桌面图标层（`kCGWindowListExcludeDesktopElements`）和层级为负的窗口（桌面背景）。
pub fn windows() -> Vec<ScreenWindow> {
    let own_pid = std::process::id() as i32;
    let mut out = Vec::new();
    // SAFETY: 返回的数组由我们持有，读完释放；字典里的值属于数组，不单独释放。
    unsafe {
        let list = CGWindowListCopyWindowInfo(
            K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
            K_CG_NULL_WINDOW_ID,
        );
        if list.is_null() {
            return out;
        }
        for index in 0..CFArrayGetCount(list) {
            let info = CFArrayGetValueAtIndex(list, index);
            let pid = number_i32(info, kCGWindowOwnerPID);
            let layer = number_i32(info, kCGWindowLayer).unwrap_or(0);
            let alpha = number_f64(info, kCGWindowAlpha).unwrap_or(1.0);
            if pid == Some(own_pid) || layer < 0 || alpha <= 0.0 {
                continue;
            }
            let bounds = CFDictionaryGetValue(info, kCGWindowBounds);
            let mut rect = CGRect::default();
            if bounds.is_null() || !CGRectMakeWithDictionaryRepresentation(bounds, &mut rect) {
                continue;
            }
            out.push(ScreenWindow {
                frame: [
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height,
                ],
                floating: layer > 0,
            });
        }
        CFRelease(list);
    }
    out
}

/// 读取字典中的整数值。
unsafe fn number_i32(dict: CFTypeRef, key: CFTypeRef) -> Option<i32> {
    let value = CFDictionaryGetValue(dict, key);
    let mut out = 0i32;
    (!value.is_null()
        && CFNumberGetValue(
            value,
            K_CF_NUMBER_SINT32_TYPE,
            (&mut out as *mut i32).cast(),
        ))
    .then_some(out)
}

/// 读取字典中的浮点值。
unsafe fn number_f64(dict: CFTypeRef, key: CFTypeRef) -> Option<f64> {
    let value = CFDictionaryGetValue(dict, key);
    let mut out = 0f64;
    (!value.is_null()
        && CFNumberGetValue(
            value,
            K_CF_NUMBER_DOUBLE_TYPE,
            (&mut out as *mut f64).cast(),
        ))
    .then_some(out)
}
