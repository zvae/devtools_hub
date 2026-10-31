//! 系统缩略图：走 shell 的缩略图提供者（`IShellItemImageFactory`）。
//!
//! 这是资源管理器显示预览的同一套接口：图片由 WIC 解码，视频取首帧，PDF / Office 走各自的
//! 缩略图处理器，装了预览器的第三方格式也一并支持。界面拿到的是已经解码好的位图，
//! 不需要我们再认识这些格式。
//!
//! 两个要点：
//! - **必须在 STA 线程调用**。这里自己 `CoInitializeEx(APARTMENTTHREADED)`，
//!   同一线程重复初始化返回 `S_FALSE`，无害；调用方只需要保证不在 UI 线程上等它。
//! - `SIIGBF_THUMBNAILONLY`：只要真正的预览，没有缩略图处理器时直接失败
//!   （由调用方回退到图标），而不是返回一个放大的文件类型图标冒充预览。

use std::mem::size_of;
use std::path::Path;

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF,
    SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY, SIIGBF_THUMBNAILONLY,
};

use super::wide;

/// 取系统缩略图（内容预览）：返回 (宽, 高, RGBA)。没有可用的预览处理器时返回 `None`。
pub fn thumbnail(path: &Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    image_for(path, size, SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK)
}

/// 取系统图标（**按请求尺寸渲染**）。
///
/// 和 `SHGetFileInfoW` 拿的 32×32 小图标不同，这里由 shell 直接按目标尺寸出图
/// （大图标时它会给高分辨率资源），所以图标视图里不会再糊成一团。
pub fn icon(path: &Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    image_for(path, size, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK)
}

/// 共同的取图流程。
fn image_for(path: &Path, size: u32, flags: SIIGBF) -> Option<(u32, u32, Vec<u8>)> {
    let size = size.clamp(16, 512);
    let wide_path = wide(&path.to_string_lossy());
    // SAFETY: 只调用 Win32 / COM 接口，所有指针都指向本函数的栈上或堆上变量，
    // 生命周期覆盖调用；位图在用完后显式删除。
    unsafe {
        // 缩略图线程是专用线程：在这里初始化 STA，重复调用返回 S_FALSE 也不报错。
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None).ok()?;
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let bitmap: HBITMAP = factory
            .GetImage(
                SIZE {
                    cx: size as i32,
                    cy: size as i32,
                },
                flags,
            )
            .ok()?;
        let pixels = bitmap_to_rgba(bitmap);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        pixels
    }
}

/// HBITMAP → (宽, 高, RGBA)。按 32 位、自上而下取出 BGRA，再换成 RGBA。
///
/// shell 返回的位图不一定带 alpha（照片类通常全不透明，PNG 预览会带），
/// 全 0 的 alpha 视为"不透明"，否则透明区域会渲染成黑色。
///
/// # Safety
/// `bitmap` 必须是有效的位图句柄，调用后由调用方负责删除。
unsafe fn bitmap_to_rgba(bitmap: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    let mut info: BITMAP = std::mem::zeroed();
    let got = GetObjectW(
        HGDIOBJ(bitmap.0),
        size_of::<BITMAP>() as i32,
        Some((&mut info as *mut BITMAP).cast()),
    );
    if got == 0 {
        return None;
    }
    let (width, height) = (info.bmWidth, info.bmHeight);
    if width <= 0 || height <= 0 {
        return None;
    }
    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() {
        return None;
    }
    let mut header: BITMAPINFO = std::mem::zeroed();
    header.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    header.bmiHeader.biWidth = width;
    // 负数高度：自上而下，取出来就不用再翻转。
    header.bmiHeader.biHeight = -height;
    header.bmiHeader.biPlanes = 1;
    header.bmiHeader.biBitCount = 32;
    header.bmiHeader.biCompression = BI_RGB.0;
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let lines = GetDIBits(
        dc,
        bitmap,
        0,
        height as u32,
        Some(pixels.as_mut_ptr().cast()),
        &mut header,
        DIB_RGB_COLORS,
    );
    let _ = DeleteDC(dc);
    if lines != height {
        return None;
    }
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2); // BGRA → RGBA
    }
    if pixels.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0) {
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
    }
    Some((width as u32, height as u32, pixels))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 从系统壁纸目录里挑一张真图片（目录里还有 `Desktop.ini`、`Thumbs.db` 之类的干扰项）。
    fn find_wallpaper() -> Option<std::path::PathBuf> {
        let mut stack = vec![std::path::PathBuf::from(r"C:\Windows\Web\Wallpaper")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let picture = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg")
                    });
                if picture {
                    return Some(path);
                }
            }
        }
        None
    }

    /// 真实系统接口：用 Windows 自带的图片验证能拿到缩略图。
    #[test]
    fn shell_provides_thumbnail_for_pictures() {
        let Some(wallpaper) = find_wallpaper() else {
            return; // 系统里没有示例壁纸时跳过
        };
        let (width, height, rgba) = thumbnail(&wallpaper, 96).expect("壁纸应有缩略图");
        assert!(width > 0 && height > 0);
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        // 不存在的路径不能 panic。
        assert!(thumbnail(Path::new(r"C:\definitely\not\here.png"), 64).is_none());
    }
}

#[cfg(all(test, windows))]
mod icon_tests {
    use super::*;

    /// 目录在图标视图里也要有系统图标（大小按请求尺寸，不是 32×32 放大）。
    #[test]
    fn shell_icon_works_for_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let sub = dir.path().join("sub");
        // 目录没有内容预览：`thumbnail`（THUMBNAILONLY）会失败，`icon` 必须成功。
        assert!(thumbnail(&sub, 192).is_none(), "目录不该有内容预览");
        let (width, height, _) = icon(&sub, 96).expect("目录应该有图标");
        assert!(width >= 64 && height >= 64, "应按请求尺寸出图：{width}x{height}");
    }
}
