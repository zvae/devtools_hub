//! 来源程序图标：从 exe 中提取指定尺寸的图标，转成 PNG。

use std::io::Cursor;
use std::mem::{size_of, zeroed};
use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetIconInfo, PrivateExtractIconsW, HICON, ICONINFO,
};

use super::wide;

/// `path` 是程序的完整路径（Windows 上 `SourceApp::bundle_id` 存的就是它）。
pub fn app_icon_png(path: &str, size: f64) -> Option<Vec<u8>> {
    if !std::path::Path::new(path).is_file() {
        return None;
    }
    let size = size.round() as i32;
    let file = wide(path);
    let mut icon: HICON = null_mut();
    // SAFETY: 只提取一个图标，句柄在转换后销毁。
    let count =
        unsafe { PrivateExtractIconsW(file.as_ptr(), 0, size, size, &mut icon, null_mut(), 1, 0) };
    if count == 0 || count == u32::MAX || icon.is_null() {
        return None;
    }
    let pixels = unsafe { icon_to_rgba(icon) };
    unsafe { DestroyIcon(icon) };
    let (width, height, rgba) = pixels?;
    let image = image::RgbaImage::from_raw(width, height, rgba)?;
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

/// HICON → (宽, 高, RGBA 像素)。
unsafe fn icon_to_rgba(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    let mut info: ICONINFO = zeroed();
    if GetIconInfo(icon, &mut info) == 0 {
        return None;
    }
    let result = (|| {
        if info.hbmColor.is_null() {
            return None; // 单色图标，不处理
        }
        let mut bitmap: BITMAP = zeroed();
        let size = size_of::<BITMAP>() as i32;
        if GetObjectW(info.hbmColor, size, (&mut bitmap as *mut BITMAP).cast()) == 0 {
            return None;
        }
        let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);
        let mut pixels = read_bitmap(info.hbmColor, width, height)?;
        // 老式图标没有 alpha 通道，透明区域记录在掩码位图里。
        if pixels.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0) {
            if let Some(mask) = read_bitmap(info.hbmMask, width, height) {
                for (pixel, mask) in pixels
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(mask.as_chunks::<4>().0)
                {
                    pixel[3] = if mask[0] == 0 { 255 } else { 0 };
                }
            }
        }
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2); // BGRA → RGBA
        }
        Some((width as u32, height as u32, pixels))
    })();
    if !info.hbmColor.is_null() {
        DeleteObject(info.hbmColor);
    }
    if !info.hbmMask.is_null() {
        DeleteObject(info.hbmMask);
    }
    result
}

/// 以 32 位、自上而下的格式读取位图像素（BGRA）。
unsafe fn read_bitmap(bitmap: HBITMAP, width: i32, height: i32) -> Option<Vec<u8>> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let dc = CreateCompatibleDC(null_mut());
    if dc.is_null() {
        return None;
    }
    let mut info: BITMAPINFO = zeroed();
    info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height; // 负数表示自上而下
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let lines = GetDIBits(
        dc,
        bitmap,
        0,
        height as u32,
        pixels.as_mut_ptr().cast(),
        &mut info,
        DIB_RGB_COLORS,
    );
    DeleteDC(dc);
    (lines == height).then_some(pixels)
}

// `windows-sys` 0.61 没有导出 `SHGetFileInfoW`（挪到了 `windows` crate），这里手动声明。
#[repr(C)]
#[allow(non_snake_case)]
struct ShFileInfoW {
    hIcon: *mut std::ffi::c_void,
    iIcon: i32,
    dwAttributes: u32,
    szDisplayName: [u16; 260],
    szTypeName: [u16; 80],
}

const SHGFI_ICON: u32 = 0x0000_0100;
const SHGFI_SMALLICON: u32 = 0x0000_0001;

// `windows-sys` 0.61 同样没有导出 `SHGetFileInfoW`，手动声明并链接 shell32。
#[link(name = "shell32")]
extern "system" {
    fn SHGetFileInfoW(
        path: *const u16,
        attributes: u32,
        info: *mut ShFileInfoW,
        info_size: u32,
        flags: u32,
    ) -> usize;
}

/// 文件 / 文件夹的系统图标（RGBA，`size` 见方）。搜索结果的图标用。
///
/// 用 `SHGetFileInfoW` 取 16×16 的 shell 图标（与资源管理器同款），
/// 需要更大尺寸时按最近邻放大（搜索结果只用到 16）。
pub fn file_icon_rgba(path: &Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let size = size.clamp(8, 64);
    let path = wide(&path.to_string_lossy());
    let mut info: ShFileInfoW = unsafe { zeroed() };
    let flags = SHGFI_ICON | SHGFI_SMALLICON;
    // SAFETY: 结构体与路径缓冲区都指向本函数的栈 / 堆变量，调用期间有效。
    let ok = unsafe {
        SHGetFileInfoW(
            path.as_ptr(),
            0,
            &mut info,
            size_of::<ShFileInfoW>() as u32,
            flags,
        )
    };
    if ok == 0 || info.hIcon.is_null() {
        return None;
    }
    // SAFETY: 图标句柄来自上面的调用，转换后立即销毁。
    let pixels = unsafe { icon_to_rgba(info.hIcon) };
    unsafe { DestroyIcon(info.hIcon) };
    let (width, height, rgba) = pixels?;
    if width == size && height == size {
        return Some((width, height, rgba));
    }
    let image = image::RgbaImage::from_raw(width, height, rgba)?;
    let resized = image::imageops::resize(&image, size, size, image::imageops::FilterType::Lanczos3);
    Some((size, size, resized.into_raw()))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn explorer_icon_is_png() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let png =
            app_icon_png(&format!(r"{windir}\explorer.exe"), 64.0).expect("资源管理器一定有图标");
        let image = image::load_from_memory(&png).unwrap();
        assert!(image.width() >= 32);
        assert!(app_icon_png(r"C:\definitely\not\here.exe", 64.0).is_none());
    }
}
