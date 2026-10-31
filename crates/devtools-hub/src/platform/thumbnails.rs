//! 图片文件的缩略图：用 `image` 解码后等比缩到目标尺寸，供搜索结果的图标视图做预览。
//!
//! 为什么不用系统缩略图（`IShellItemImageFactory` / QuickLook）：那套接口要额外的 COM / objc
//! 绑定，而且必须跑在有窗口会话的线程上；图片文件用 `image` 直接解码已经能得到与资源管理器
//! 一致的预览，还不依赖 shell 的缩略图缓存。代价是**只覆盖图片**——视频、PDF、Office 文档
//! 的预览仍然回退到系统图标（见 `module` 里的说明）。
//!
//! 解码很慢（一张 12MP 的 JPEG 几十到上百毫秒），**只能在后台线程调用**，
//! 搜索页的 `ThumbnailCache` 就是干这个的。

use std::path::Path;

/// 超过这个大小的文件不生成缩略图：解码一次会把后台线程占住好几秒，
/// 而这么大的文件多半也不是用户想"预览"的对象。
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// 解码的宽/高上限（防止异常图片把内存拉爆）。
const MAX_DIMENSION: u32 = 16_384;
/// 解码允许申请的最大内存。
const MAX_ALLOC: u64 = 256 * 1024 * 1024;

/// `image` 编译进来的格式（与根 `Cargo.toml` 的 features 一致）。
/// 视频 / PDF / Office 不在其中，界面会回退到系统图标。
const PICTURE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "gif", "bmp", "dib", "webp", "tif", "tiff",
];

/// 这个文件是否值得尝试生成缩略图（按扩展名判断，不做 IO）。
pub fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .is_some_and(|ext| PICTURE_EXTENSIONS.contains(&ext.as_str()))
}

/// 生成缩略图：返回 (宽, 高, RGBA)。保持宽高比，不放大比 `size` 还小的图。
///
/// 读不到、不是图片、格式不支持、解码失败都返回 `None`（界面回退到系统图标）。
pub fn image_thumbnail(path: &Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    if !is_picture(path) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let size = size.clamp(16, 512);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC);
    let mut reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    // `limits` 是 `&mut self`，不能串进链式调用（返回值是 ()）。
    reader.limits(limits);
    let image = reader.decode().ok()?;
    // 按比例缩到 size 见方以内；本来就比目标小的图保持原样（放大只会变糊还费内存）。
    let thumb = if image.width() <= size && image.height() <= size {
        image
    } else {
        image.thumbnail(size, size)
    };
    let rgba = thumb.to_rgba8();
    Some((rgba.width(), rgba.height(), rgba.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(dir: &Path, name: &str, width: u32, height: u32) -> std::path::PathBuf {
        let path = dir.join(name);
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([30, 120, 220, 255]));
        image.save(&path).unwrap();
        path
    }

    #[test]
    fn thumbnails_keep_aspect_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "wide.png", 400, 200);
        let (width, height, rgba) = image_thumbnail(&path, 96).expect("应能生成缩略图");
        assert_eq!((width, height), (96, 48), "保持 2:1 的宽高比");
        assert_eq!(rgba.len(), (width * height * 4) as usize);
    }

    #[test]
    fn small_pictures_are_not_upscaled() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "small.png", 40, 30);
        let (width, height, _) = image_thumbnail(&path, 128).unwrap();
        assert_eq!((width, height), (40, 30));
    }

    #[test]
    fn non_pictures_and_missing_files_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "hello").unwrap();
        assert!(image_thumbnail(&text, 64).is_none());
        assert!(!is_picture(&text));
        assert!(image_thumbnail(&dir.path().join("missing.png"), 64).is_none());
        // 扩展名是图片但内容不是：解码失败也只是回退，不 panic。
        let broken = dir.path().join("broken.jpg");
        std::fs::write(&broken, b"not a jpeg").unwrap();
        assert!(image_thumbnail(&broken, 64).is_none());
    }
}
