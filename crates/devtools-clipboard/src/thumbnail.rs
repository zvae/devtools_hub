//! 图片缩略图（对应 PasteNow 的 `ZPASTEITEM.ZTHUMBNAILIMAGE`，同样存为 PNG）。

use std::io::Cursor;

use anyhow::{Context, Result};
use image::{ImageFormat, ImageReader};

/// 缩略图最长边（像素）。预览区也直接使用它，避免每次都解码原图。
pub const MAX_SIDE: u32 = 640;

/// 生成的缩略图。
#[derive(Clone, Debug)]
pub struct Thumbnail {
    pub png: Vec<u8>,
    /// 原图尺寸。
    pub width: u32,
    pub height: u32,
}

/// 解码图片（自动识别格式），缩小到最长边不超过 [`MAX_SIDE`]，编码成 PNG。
pub fn make(bytes: &[u8]) -> Result<Thumbnail> {
    let image = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("无法识别图片格式")?
        .decode()
        .context("解码图片失败")?;
    let (width, height) = (image.width(), image.height());
    let thumb = if width > MAX_SIDE || height > MAX_SIDE {
        image.thumbnail(MAX_SIDE, MAX_SIDE)
    } else {
        image
    };
    let mut png = Vec::new();
    thumb
        .to_rgba8()
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .context("编码缩略图失败")?;
    Ok(Thumbnail { png, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn encode(img: &RgbaImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), format).unwrap();
        out
    }

    #[test]
    fn shrinks_large_tiff() {
        let img = RgbaImage::from_pixel(1600, 800, Rgba([10, 20, 30, 255]));
        let thumb = make(&encode(&img, ImageFormat::Tiff)).unwrap();
        assert_eq!((thumb.width, thumb.height), (1600, 800));
        let decoded = image::load_from_memory(&thumb.png).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (MAX_SIDE, MAX_SIDE / 2)
        );
    }

    #[test]
    fn keeps_small_png() {
        let img = RgbaImage::from_pixel(20, 10, Rgba([0, 0, 0, 255]));
        let thumb = make(&encode(&img, ImageFormat::Png)).unwrap();
        let decoded = image::load_from_memory(&thumb.png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (20, 10));
    }

    #[test]
    fn rejects_garbage() {
        assert!(make(b"definitely not an image").is_err());
    }
}
