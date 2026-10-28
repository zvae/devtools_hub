//! 文字识别：Vision 的 `VNRecognizeTextRequest`（准确模式，简体中文 + 英文）。
//!
//! 中文识别从 macOS 11 起支持（项目最低 12.0）。`performRequests` 是同步的，在后台线程调用；
//! 大图、长截图按高度分块识别（见 `snip::ocr`），整张交给 Vision 时小字会被缩小到认不出。

use anyhow::{anyhow, Result};
use egui::{pos2, vec2, Rect};
use image::RgbaImage;
use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel, AnyThread};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSRange, NSRect, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedText, VNRequest,
    VNRequestTextRecognitionLevel,
};

use crate::snip::{self, ocr::Line};

/// 每块最高多少像素（不低于 [`MIN_TILE`]，不高于 [`MAX_TILE`]，其间与图片一样宽）。
/// Vision 会把整张输入缩小到内部的分辨率，图片太大时小字被压得认不出：实测 16 像素高的文字，
/// 1200 宽的图片高 2000 时全部认得出、高 3000 时只认出一半，2400 宽的图片高 2000 时一行也认不出。
const MIN_TILE: u32 = 800;
const MAX_TILE: u32 = 1600;
/// 相邻两块重叠的像素数（比一行字高得多，被切断的行在另一块里是完整的）。
const OVERLAP: u32 = 240;
/// 识别的语言，按优先级排列。
const LANGUAGES: [&str; 2] = ["zh-Hans", "en-US"];

/// 识别图片中的文字，按阅读顺序返回每一行（坐标是图片中的像素）。
pub fn recognize(image: &RgbaImage) -> Result<Vec<Line>> {
    let tile = image.width().clamp(MIN_TILE, MAX_TILE);
    snip::ocr::recognize_tiled(image, tile, OVERLAP, recognize_tile)
}

fn recognize_tile(image: &RgbaImage) -> Result<Vec<Line>> {
    // Vision 从 PNG 数据读入图片，省去自己构造 CGImage（编码一张屏幕大小的图只要几十毫秒）。
    let png = snip::encode_png(image, true)?;
    let (width, height) = (image.width() as f64, image.height() as f64);
    autoreleasepool(|_| {
        let data = NSData::with_bytes(&png);
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &NSDictionary::new(),
        );
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        let languages: Vec<_> = LANGUAGES.iter().map(|l| NSString::from_str(l)).collect();
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));
        let requests: [&VNRequest; 1] = [&request];
        handler
            .performRequests_error(&NSArray::from_slice(&requests))
            .map_err(|err| anyhow!("Vision 识别失败：{}", err.localizedDescription()))?;
        let Some(results) = request.results() else {
            return Ok(Vec::new());
        };
        let mut lines = Vec::new();
        for observation in results.iter() {
            let Some(candidate) = observation.topCandidates(1).firstObject() else {
                continue;
            };
            // 归一化的坐标，原点在左下角。
            // SAFETY: `boundingBox` 只读取观测结果的属性。
            let bounds = unsafe { observation.boundingBox() };
            let min = pos2(
                (bounds.origin.x * width) as f32,
                ((1.0 - bounds.origin.y - bounds.size.height) * height) as f32,
            );
            let size = vec2(
                (bounds.size.width * width) as f32,
                (bounds.size.height * height) as f32,
            );
            let rect = Rect::from_min_size(min, size);
            let mut line = Line::new(candidate.string().to_string(), rect);
            line.chars = char_boxes(&candidate, width, height);
            lines.push(line);
        }
        Ok(lines)
    })
}

/// 一行里每个字符的位置：Vision 能按字符范围给出矩形（`boundingBoxForRange:error:`），
/// 拿不到时返回空，由 [`crate::snip::ocr::fill_chars`] 按字符宽度平铺。
fn char_boxes(candidate: &VNRecognizedText, width: f64, height: f64) -> Vec<Rect> {
    // SAFETY: 只查询方法是否存在，没有参数。
    let supported = unsafe {
        let responds: bool =
            msg_send![candidate, respondsToSelector: sel!(boundingBoxForRange:error:)];
        responds
    };
    if !supported {
        return Vec::new();
    }
    let text = candidate.string().to_string();
    let mut boxes = Vec::with_capacity(text.chars().count());
    // `NSRange` 按 UTF-16 计数：一个汉字是一位，emoji 这类是两位。
    let mut location = 0;
    for c in text.chars() {
        let length = c.len_utf16();
        // SAFETY: `boundingBoxForRange:error:` 的参数是 NSRange（值）与 NSError 指针（可以为空）；
        // 返回的是 autoreleased 的 VNRectangleObservation，本函数在调用方的 autoreleasepool 里读完。
        unsafe {
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let observation: *mut AnyObject = msg_send![
                candidate,
                boundingBoxForRange: NSRange::new(location, length),
                error: &mut error
            ];
            location += length;
            if observation.is_null() {
                return Vec::new();
            }
            let bounds: NSRect = msg_send![observation, boundingBox];
            boxes.push(Rect::from_min_size(
                pos2(
                    (bounds.origin.x * width) as f32,
                    ((1.0 - bounds.origin.y - bounds.size.height) * height) as f32,
                ),
                vec2(
                    (bounds.size.width * width) as f32,
                    (bounds.size.height * height) as f32,
                ),
            ));
        }
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用界面截图测试真实的识别（需要 Vision，只在本机手动运行）：
    /// `DEVTOOLS_HUB_OCR_IMAGE=图片路径 cargo test -p devtools-hub ocr_image -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn ocr_image() {
        let path = std::env::var("DEVTOOLS_HUB_OCR_IMAGE").expect("设置 DEVTOOLS_HUB_OCR_IMAGE");
        let image = image::open(path).unwrap().to_rgba8();
        let started = std::time::Instant::now();
        let lines = recognize(&image).unwrap();
        println!("{} 行，用时 {:?}", lines.len(), started.elapsed());
        println!("{}", snip::ocr::to_text(&lines));
        assert!(!lines.is_empty());
        // 每个字符都有位置（Vision 的 `boundingBoxForRange:`；拿不到时按宽度平铺）。
        let chars = lines[0].chars.len();
        println!(
            "第一行“{}”：{chars} 个字符位置，前两个 {:?}",
            lines[0].text,
            &lines[0].chars[..chars.min(2)]
        );
        assert_eq!(chars, lines[0].text.chars().count());

        // 把图片顶部的一条（`DEVTOOLS_HUB_OCR_STRIP` 像素高）重复拼成长图：每一行都只识别一次。
        let Some(strip) = std::env::var("DEVTOOLS_HUB_OCR_STRIP")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            return;
        };
        let once =
            recognize(&image::imageops::crop_imm(&image, 0, 0, image.width(), strip).to_image())
                .unwrap()
                .len();
        let repeat = MAX_TILE * 5 / strip;
        let mut long = RgbaImage::new(image.width(), strip * repeat);
        for index in 0..repeat {
            let part = image::imageops::crop_imm(&image, 0, 0, image.width(), strip);
            image::imageops::replace(&mut long, &*part, 0, i64::from(index * strip));
        }
        let started = std::time::Instant::now();
        let lines = recognize(&long).unwrap();
        println!(
            "长图 {}×{}：{} 行（每条 {once} 行 × {repeat}），用时 {:?}",
            long.width(),
            long.height(),
            lines.len(),
            started.elapsed()
        );
        assert_eq!(lines.len(), once * repeat as usize);
    }
}
