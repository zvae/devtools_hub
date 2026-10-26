//! 文字识别：WinRT 的 `Windows.Media.Ocr`。
//!
//! 优先用简体中文的识别引擎（中文引擎也认得英文，英文引擎认不出中文），系统没装中文 OCR 语言组件时
//! 用用户语言列表里的第一个可用语言。引擎限制了图片的最大边长（`MaxImageDimension`）：
//! 比它宽的图片先缩小，比它高的长截图按高度分块识别（见 `snip::ocr`）。
//!
//! 识别结果以“单词”为单位，每个汉字是一个单词，单词之间用空格连接，合并时去掉汉字之间的空格。

use anyhow::{anyhow, Context, Result};
use egui::{pos2, Rect};
use image::RgbaImage;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

use crate::snip::{self, ocr::Line};

/// 优先使用的识别语言。
const LANGUAGES: [&str; 3] = ["zh-Hans-CN", "zh-Hans", "zh-Hant"];
/// 相邻两块重叠的像素数。
const OVERLAP: u32 = 240;

/// 识别图片中的文字，按阅读顺序返回每一行（坐标是图片中的像素）。
pub fn recognize(image: &RgbaImage) -> Result<Vec<Line>> {
    // SAFETY: 在当前（后台识别）线程初始化 WinRT；已经初始化过时返回的错误可以忽略。
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let engine = engine()?;
    let max = OcrEngine::MaxImageDimension()
        .unwrap_or(2600)
        .max(OVERLAP * 4);
    // 太宽的图片按比例缩小到引擎允许的宽度，识别后把坐标放大回去。
    let scale = (max as f32 / image.width() as f32).min(1.0);
    let scaled;
    let input = if scale < 1.0 {
        let height = ((image.height() as f32 * scale).round() as u32).max(1);
        scaled = image::imageops::resize(image, max, height, image::imageops::FilterType::Triangle);
        &scaled
    } else {
        image
    };
    let lines =
        snip::ocr::recognize_tiled(input, max, OVERLAP, |tile| recognize_tile(&engine, tile))?;
    Ok(lines
        .into_iter()
        .map(|line| Line {
            rect: Rect::from_min_max(
                (line.rect.min.to_vec2() / scale).to_pos2(),
                (line.rect.max.to_vec2() / scale).to_pos2(),
            ),
            ..line
        })
        .collect())
}

/// 创建识别引擎：优先中文，没有中文语言组件时用用户的语言。
fn engine() -> Result<OcrEngine> {
    for tag in LANGUAGES {
        let Ok(language) = Language::CreateLanguage(&HSTRING::from(tag)) else {
            continue;
        };
        if OcrEngine::IsLanguageSupported(&language).unwrap_or(false) {
            if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&language) {
                return Ok(engine);
            }
        }
    }
    tracing::info!("系统没有中文 OCR 语言组件，改用用户语言列表中的语言");
    OcrEngine::TryCreateFromUserProfileLanguages()
        .context("系统没有可用的 OCR 语言组件（设置 → 时间和语言 → 语言和区域 → 添加语言时勾选“光学字符识别”）")
}

fn recognize_tile(engine: &OcrEngine, image: &RgbaImage) -> Result<Vec<Line>> {
    let mut bgra = image.as_raw().clone();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let buffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        image.width() as i32,
        image.height() as i32,
    )?;
    let result = engine
        .RecognizeAsync(&bitmap)?
        .join()
        .map_err(|err| anyhow!("识别失败：{err}"))?;
    let mut lines = Vec::new();
    for line in result.Lines()? {
        let text = snip::ocr::remove_cjk_spaces(&line.Text()?.to_string_lossy());
        let mut rect: Option<Rect> = None;
        for word in line.Words()? {
            let bounds = word.BoundingRect()?;
            let word_rect = Rect::from_min_max(
                pos2(bounds.X, bounds.Y),
                pos2(bounds.X + bounds.Width, bounds.Y + bounds.Height),
            );
            rect = Some(rect.map_or(word_rect, |rect| rect.union(word_rect)));
        }
        if let Some(rect) = rect {
            // 字符的位置由 `snip::ocr::fill_chars` 按字符宽度平铺（Windows 只给单词的框）。
            lines.push(Line::new(text, rect));
        }
    }
    Ok(lines)
}
