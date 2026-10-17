//! 从 IDE 复制的代码按富文本里的高亮显示（与 PasteNow 一致），字体统一用等宽字体。
//!
//! 富文本里的颜色是按 IDE 的主题选的：深色主题的浅色文字放在浅色面板上会看不清，
//! 对比度不够时翻转亮度（保留色相），见 [`readable`]。

use std::collections::HashMap;
use std::sync::Arc;

use devtools_clipboard::styled::{self, StyledRun};
use devtools_clipboard::{ClipItem, ContentKind, Store};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, FontId, Rgba};

use super::style::Palette;

/// 缓存的条目数上限，超过后整个清空重建。
const MAX_ENTRIES: usize = 300;
/// 最多解析的字符数，与大预览的上限一致。
const MAX_CHARS: usize = 20_000;
/// 卡片上最多排版的字符数（只显示两行）。
const CARD_CHARS: usize = 400;
/// 文字与背景至少要有的对比度（WCAG 对大号文字、图形的要求）。
const MIN_CONTRAST: f32 = 3.0;

/// 条目 id → 代码样式（不是代码的也缓存为 `None`）。
#[derive(Default)]
pub struct CodeStyles {
    map: HashMap<i64, Option<Arc<Vec<StyledRun>>>>,
}

impl CodeStyles {
    /// 富文本条目的代码样式（只解析一次，不是代码时也缓存为 `None`）。
    pub fn get(&mut self, store: &Store, item: &ClipItem) -> Option<Arc<Vec<StyledRun>>> {
        if item.kind != ContentKind::RichText {
            return None;
        }
        if let Some(runs) = self.map.get(&item.id) {
            return runs.clone();
        }
        if self.map.len() >= MAX_ENTRIES {
            self.map.clear();
        }
        let runs = store
            .snapshot(item.id)
            .ok()
            .and_then(|snapshot| styled::code_runs(&snapshot, MAX_CHARS))
            .map(Arc::new);
        self.map.insert(item.id, runs.clone());
        runs
    }

    /// 条目被删除时清掉缓存。
    pub fn forget(&mut self, id: i64) {
        self.map.remove(&id);
    }
}

/// 卡片上的预览：与纯文本的标题一样把空白折叠成一个空格，最多 `max_rows` 行。
pub fn card_job(
    runs: &[StyledRun],
    palette: &Palette,
    size: f32,
    width: f32,
    max_rows: usize,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut count = 0;
    let mut pending_space = false;
    'runs: for run in runs {
        let format = format(run, palette, size);
        let mut text = String::new();
        for ch in run.text.chars() {
            if ch.is_whitespace() {
                pending_space = count > 0;
                continue;
            }
            if pending_space {
                text.push(' ');
                pending_space = false;
            }
            text.push(ch);
            count += 1;
            if count >= CARD_CHARS {
                job.append(&text, 0.0, format);
                break 'runs;
            }
        }
        job.append(&text, 0.0, format);
    }
    // 与纯文本卡片一样优先在空格处换行。
    job.wrap = TextWrapping {
        max_width: width,
        max_rows,
        break_anywhere: max_rows == 1,
        ..Default::default()
    };
    job
}

/// 大预览：保留换行与缩进，不自动换行。
pub fn preview_job(runs: &[StyledRun], palette: &Palette, size: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    for run in runs {
        job.append(&run.text, 0.0, format(run, palette, size));
    }
    job
}

/// 一段代码的文字格式：等宽字体，颜色调整到在当前背景上看得清。
fn format(run: &StyledRun, palette: &Palette, size: f32) -> TextFormat {
    TextFormat {
        font_id: FontId::monospace(size),
        color: readable(run.color, palette),
        italics: run.italic,
        ..Default::default()
    }
}

/// 富文本里的颜色在当前面板上的显示颜色：对比度够就用原色，不够就翻转亮度，还不够就用正文色。
pub fn readable(color: Option<[u8; 3]>, palette: &Palette) -> Color32 {
    let Some([r, g, b]) = color else {
        return palette.text;
    };
    // 卡片的底色：深色是窗口底色叠一层浅白（约 #2b2b2d），浅色接近纯白。
    let dark = luminance(palette.text) > 0.5;
    let background = if dark {
        Color32::from_rgb(43, 43, 45)
    } else {
        Color32::from_rgb(254, 254, 254)
    };
    let original = Color32::from_rgb(r, g, b);
    [original, invert_lightness(original)]
        .into_iter()
        .find(|&color| contrast(color, background) >= MIN_CONTRAST)
        .unwrap_or(palette.text)
}

/// 相对亮度（WCAG 定义，按线性 RGB 计算）。
fn luminance(color: Color32) -> f32 {
    let linear = Rgba::from(color);
    0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b()
}

/// 两种颜色的对比度（WCAG 定义，1–21）。
fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// 在 HSL 中把亮度 L 换成 1 − L，色相与饱和度不变。
fn invert_lightness(color: Color32) -> Color32 {
    let [r, g, b] = [color.r(), color.g(), color.b()].map(|c| f32::from(c) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    let (hue, saturation) = if delta == 0.0 {
        (0.0, 0.0)
    } else {
        let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
        let hue = if max == r {
            ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };
        (hue * 60.0, saturation)
    };
    let lightness = 1.0 - lightness;
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let x = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    let channel = |value: f32| ((value + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(channel(r), channel(g), channel(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Backdrop;

    #[test]
    fn keeps_readable_colors_and_flips_unreadable_ones() {
        let dark = Palette::new(true, Backdrop::Opaque, 0);
        let light = Palette::new(false, Backdrop::Opaque, 0);
        // Darcula 的关键字色在深色面板上够清楚，原样保留。
        let keyword = Color32::from_rgb(204, 120, 50);
        assert_eq!(readable(Some([204, 120, 50]), &dark), keyword);
        // 深色主题的默认文字色（浅灰）放到浅色面板上：翻转成深灰。
        let flipped = readable(Some([212, 212, 212]), &light);
        assert!(contrast(flipped, Color32::WHITE) >= MIN_CONTRAST);
        assert!(flipped.r() < 80 && flipped.r() == flipped.g());
        // 浅色主题的深蓝关键字放到深色面板上：翻转成浅蓝，色相不变。
        let blue = readable(Some([0, 51, 179]), &dark);
        assert!(blue.b() > blue.r() && blue.b() > 180);
        // 没有颜色时用正文色。
        assert_eq!(readable(None, &dark), dark.text);
    }

    #[test]
    fn card_collapses_whitespace() {
        let palette = Palette::new(true, Backdrop::Opaque, 0);
        let run = |text: &str| StyledRun {
            text: text.into(),
            color: None,
            bold: false,
            italic: false,
        };
        let job = card_job(
            &[run("fn main() {\n    "), run("let a"), run(" = 1;\n}\n")],
            &palette,
            12.5,
            200.0,
            2,
        );
        assert_eq!(job.text, "fn main() { let a = 1; }");
    }
}
