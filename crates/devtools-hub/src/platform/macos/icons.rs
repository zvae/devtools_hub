//! 来源应用图标与文件图标（PasteNow 把图标存在 `ZAPPLICATION.ZICON`，这里按需从系统读取并缓存）。

use std::path::Path;

use objc2::rc::autoreleasepool;
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

/// 返回应用图标的 PNG 数据，边长约为 `size` 像素。找不到应用时返回 `None`。
pub fn app_icon_png(bundle_id: &str, size: f64) -> Option<Vec<u8>> {
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        let url =
            workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
        let path = url.path()?;
        let image = workspace.iconForFile(&path);
        let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(size, size));
        // SAFETY: rect 指向有效的栈变量；不传上下文和 hints 时系统选用最接近该尺寸的表示。
        let cg_image =
            unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
        // SAFETY: 空属性字典是合法参数。
        let png = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }?;
        Some(png.to_vec())
    })
}

/// 文件 / 文件夹的系统图标（RGBA，`size` 见方）。搜索结果的图标用。
///
/// 走 `NSWorkspace::iconForFile`，得到的是系统为该文件类型绘制的图标（Finder 同款，
/// 对 `.app`、磁盘映像、Office 文档等都会用各自的应用图标）。
pub fn file_icon_rgba(path: &Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let size = size.clamp(8, 256);
    let path = NSString::from_str(path.to_str()?);
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        let image = workspace.iconForFile(&path);
        let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(size as f64, size as f64));
        // SAFETY: rect 指向有效的栈变量；不传上下文和 hints 时系统选用最接近该尺寸的表示。
        let cg_image =
            unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
        // SAFETY: 空属性字典是合法参数。
        let png = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }?;
        let decoded = image::load_from_memory(&png.to_vec()).ok()?.into_rgba8();
        let resized = image::imageops::resize(
            &decoded,
            size,
            size,
            image::imageops::FilterType::Lanczos3,
        );
        Some((size, size, resized.into_raw()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_icon_is_rgba() {
        let (width, height, pixels) =
            file_icon_rgba(Path::new("/System/Library/CoreServices/Finder.app"), 16)
                .expect("访达的图标一定存在");
        assert_eq!((width, height), (16, 16));
        assert_eq!(pixels.len(), 16 * 16 * 4);
        // 图标不是全透明。
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
        // 不存在的路径会拿到系统"通用文档"图标（NSWorkspace 不返回 None），尺寸仍然正确。
        let (width, height, _) = file_icon_rgba(Path::new("/definitely/not/here"), 16).unwrap();
        assert_eq!((width, height), (16, 16));
    }

    #[test]
    fn finder_icon_is_png() {
        let png = app_icon_png("com.apple.finder", 64.0).expect("访达一定存在");
        let image = image::load_from_memory(&png).unwrap();
        assert!(
            image.width() >= 32 && image.width() <= 256,
            "宽度 {}",
            image.width()
        );
        assert!(app_icon_png("com.example.definitely-not-installed", 64.0).is_none());
    }
}
