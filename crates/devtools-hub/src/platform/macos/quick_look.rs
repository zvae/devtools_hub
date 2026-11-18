//! 在搜索缩略图线程上向 Quick Look 请求文件内容的预览。

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::{autoreleasepool, Allocated, Retained};
use objc2::{extern_class, extern_methods, AnyThread};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSError, NSObject, NSSize, NSURL};

#[link(name = "QuickLookThumbnailing", kind = "framework")]
extern "C" {}

extern_class!(
    #[unsafe(super(NSObject))]
    struct QLThumbnailGenerator;
);
extern_class!(
    #[unsafe(super(NSObject))]
    struct QLThumbnailGenerationRequest;
);
extern_class!(
    #[unsafe(super(NSObject))]
    struct QLThumbnailRepresentation;
);

type Completion = block2::DynBlock<dyn Fn(*mut QLThumbnailRepresentation, *mut NSError)>;

impl QLThumbnailGenerator {
    extern_methods!(
        #[unsafe(method(sharedGenerator))]
        #[unsafe(method_family = none)]
        fn shared() -> Retained<Self>;

        #[unsafe(method(generateBestRepresentationForRequest:completionHandler:))]
        #[unsafe(method_family = none)]
        fn generate(&self, request: &QLThumbnailGenerationRequest, completion: &Completion);

        #[unsafe(method(cancelRequest:))]
        #[unsafe(method_family = none)]
        fn cancel(&self, request: &QLThumbnailGenerationRequest);
    );
}

impl QLThumbnailGenerationRequest {
    extern_methods!(
        #[unsafe(method(initWithFileAtURL:size:scale:representationTypes:))]
        #[unsafe(method_family = init)]
        fn init(
            this: Allocated<Self>,
            url: &NSURL,
            size: NSSize,
            scale: f64,
            representation_types: usize,
        ) -> Retained<Self>;
    );
}

impl QLThumbnailRepresentation {
    extern_methods!(
        #[unsafe(method(CGImage))]
        #[unsafe(method_family = none)]
        fn image(&self) -> *const CGImage;
    );
}

// 对应 QLThumbnailGenerationRequestRepresentationTypeThumbnail，仅请求内容缩略图。
const CONTENT_THUMBNAIL: usize = 1 << 2;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

pub(in crate::platform) fn thumbnail(path: &Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let size = size.clamp(16, 512);
    autoreleasepool(|_| {
        // 安全性：path 是有效的 NUL 结尾文件系统路径，NSURL 会复制路径数据。
        let url = unsafe {
            NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
                NonNull::new(path.as_ptr().cast_mut())?,
                false,
                None,
            )
        };
        let generator = QLThumbnailGenerator::shared();
        let request = QLThumbnailGenerationRequest::init(
            QLThumbnailGenerationRequest::alloc(),
            &url,
            NSSize::new(f64::from(size), f64::from(size)),
            1.0, // 调用方已经把 DPI 计入像素尺寸。
            CONTENT_THUMBNAIL,
        );
        let (send, receive) = mpsc::sync_channel(1);
        let completion = RcBlock::new(
            move |representation: *mut QLThumbnailRepresentation, _: *mut NSError| {
                autoreleasepool(|_| {
                    // 安全性：系统在回调期间持有 representation 和它的 CGImage。
                    let pixels =
                        unsafe { representation.as_ref().and_then(|r| r.image().as_ref()) }
                            .and_then(|image| super::icons::cg_image_png(image))
                            .and_then(|png| {
                                image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                                    .ok()
                            })
                            .map(|image| {
                                let image = if image.width() > size || image.height() > size {
                                    image.thumbnail(size, size)
                                } else {
                                    image
                                };
                                let pixels = image.into_rgba8();
                                (pixels.width(), pixels.height(), pixels.into_raw())
                            });
                    let _ = send.try_send(pixels);
                });
            },
        );
        generator.generate(&request, &completion);
        match receive.recv_timeout(REQUEST_TIMEOUT) {
            Ok(thumbnail) => thumbnail,
            Err(_) => {
                generator.cancel(&request);
                None
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn system_thumbnail(path: std::path::PathBuf) -> (u32, u32, Vec<u8>) {
        std::thread::spawn(move || thumbnail(&path, 96))
            .join()
            .unwrap()
            .expect("Quick Look 应返回内容缩略图")
    }

    fn has_red_content(rgba: &[u8]) -> bool {
        rgba.chunks_exact(4)
            .any(|pixel| pixel[0] > 180 && pixel[1] < 65 && pixel[3] == 255)
    }

    #[test]
    #[ignore = "需要 macOS Quick Look 服务，手动运行"]
    fn system_thumbnail_contains_image_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("内容预览.png");
        image::RgbaImage::from_pixel(240, 120, image::Rgba([212, 40, 70, 255]))
            .save(&path)
            .unwrap();
        let (w, h, rgba) = system_thumbnail(path);
        assert_eq!((w, h), (96, 48));
        assert!(has_red_content(&rgba));
    }

    #[test]
    #[ignore = "需要 macOS Quick Look 服务，手动运行"]
    fn system_thumbnail_contains_pdf_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("内容预览.pdf");
        let page = "0.83 0.16 0.27 rg 0 0 100 100 re f\n";
        let stream = format!("<< /Length {} >>\nstream\n{page}endstream", page.len());
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << >> /Contents 4 0 R >>",
            &stream,
        ];
        for (i, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            writeln!(pdf, "{} 0 obj\n{object}\nendobj", i + 1).unwrap();
        }
        let xref = pdf.len();
        write!(pdf, "xref\n0 5\n0000000000 65535 f \n").unwrap();
        for offset in offsets {
            writeln!(pdf, "{offset:010} 00000 n ").unwrap();
        }
        writeln!(
            pdf,
            "trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF"
        )
        .unwrap();
        std::fs::write(&path, pdf).unwrap();
        let (w, h, rgba) = system_thumbnail(path);
        assert!(w <= 96 && h <= 96);
        assert!(has_red_content(&rgba));
    }
}
