//! 来源应用图标（PasteNow 把图标存在 `ZAPPLICATION.ZICON`，这里按需从系统读取并缓存）。

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

#[cfg(test)]
mod tests {
    use super::*;

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
