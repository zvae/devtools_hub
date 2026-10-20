//! 文字识别（OCR）中与平台无关的部分（方案见 `docs/微信滚动截图与文字提取实现分析.md` 的 4.2 节）：
//!
//! 识别本身交给系统（macOS 的 Vision、Windows 的 `Windows.Media.Ocr`，在 `platform::ocr` 里），这里负责：
//!
//! - 分块：长截图比屏幕高得多，整张交给系统时会被缩小到看不清，按高度切成有重叠的几块分别识别，
//!   每一行只保留在它中心所在的那一块里的结果（被切断的半行在另一块里是完整的）；
//! - 阅读顺序：把识别出的文字行按行（上下位置重叠）分组，组内从左到右；
//! - 合并成文本：行与行之间换行，同一行的几段用空格隔开（离得远的用制表符）；
//! - 去掉 Windows 在汉字之间插入的空格（它按“单词”识别，每个汉字是一个单词）。

use anyhow::Result;
use egui::{vec2, Rect};
use image::RgbaImage;

/// 识别出的一行文字。
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    /// 在图片中的位置（像素，左上角为原点）。
    pub rect: Rect,
}

/// 一块要单独识别的区域（像素行号，左闭右开）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    /// 这一块的范围。
    pub start: u32,
    pub end: u32,
    /// 中心落在这个范围里的文字行算这一块的（相邻两块的范围首尾相接，不重不漏）。
    pub own_start: u32,
    pub own_end: u32,
}

/// 把高 `height` 的图片切成每块最高 `tile`、相邻两块重叠 `overlap` 的几块。不超过 `tile` 时只有一块。
pub fn tiles(height: u32, tile: u32, overlap: u32) -> Vec<Tile> {
    let tile = tile.max(1);
    let overlap = overlap.min(tile / 2);
    if height <= tile {
        return vec![Tile {
            start: 0,
            end: height,
            own_start: 0,
            own_end: height,
        }];
    }
    let stride = tile - overlap;
    let mut result = Vec::new();
    let mut start = 0;
    loop {
        // 最后一块贴着底边，保持完整的高度（不留一条很矮的尾巴）。
        let end = (start + tile).min(height);
        let start_here = if end == height {
            height.saturating_sub(tile)
        } else {
            start
        };
        result.push(Tile {
            start: start_here,
            end,
            own_start: 0,
            own_end: height,
        });
        if end == height {
            break;
        }
        start += stride;
    }
    // 相邻两块的分界取在它们重叠部分的正中。
    for index in 1..result.len() {
        let middle = (result[index].start + result[index - 1].end) / 2;
        result[index - 1].own_end = middle;
        result[index].own_start = middle;
    }
    result
}

/// 分块识别整张图片：每块交给 `engine`，结果换算回整张图片的坐标，按阅读顺序排好。
pub fn recognize_tiled(
    image: &RgbaImage,
    tile: u32,
    overlap: u32,
    mut engine: impl FnMut(&RgbaImage) -> Result<Vec<Line>>,
) -> Result<Vec<Line>> {
    let parts = tiles(image.height(), tile, overlap);
    let mut lines = Vec::new();
    if parts.len() == 1 {
        lines = engine(image)?;
    } else {
        for part in parts {
            let crop = image::imageops::crop_imm(
                image,
                0,
                part.start,
                image.width(),
                part.end - part.start,
            )
            .to_image();
            let offset = vec2(0.0, part.start as f32);
            lines.extend(
                engine(&crop)?
                    .into_iter()
                    .map(|line| Line {
                        rect: line.rect.translate(offset),
                        ..line
                    })
                    .filter(|line| {
                        let center = line.rect.center().y;
                        center >= part.own_start as f32 && center < part.own_end as f32
                    }),
            );
        }
    }
    lines.retain(|line| !line.text.trim().is_empty());
    Ok(reading_order(lines))
}

/// 按阅读顺序排列：上下位置重叠超过矮的那一段一半的文字行算同一行，同一行内从左到右。
pub fn reading_order(lines: Vec<Line>) -> Vec<Line> {
    rows(lines).into_iter().flatten().collect()
}

/// 把文字行分成一行行（每行内从左到右）。
fn rows(mut lines: Vec<Line>) -> Vec<Vec<Line>> {
    lines.sort_by(|a, b| a.rect.top().total_cmp(&b.rect.top()));
    let mut rows: Vec<(Rect, Vec<Line>)> = Vec::new();
    for line in lines {
        let same_row = rows.last().is_some_and(|(span, _)| {
            let overlap = span.bottom().min(line.rect.bottom()) - span.top().max(line.rect.top());
            overlap > span.height().min(line.rect.height()) * 0.5
        });
        match rows.last_mut() {
            Some((span, row)) if same_row => {
                *span = span.union(line.rect);
                row.push(line);
            }
            _ => rows.push((line.rect, vec![line])),
        }
    }
    rows.into_iter()
        .map(|(_, mut row)| {
            row.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
            row
        })
        .collect()
}

/// 合并成纯文本：行与行之间换行；同一行的几段离得近时用空格隔开，离得远（超过两个字高，
/// 多半是表格的不同列或并排的两栏）时用制表符隔开，粘贴到表格里正好分列。
pub fn to_text(lines: &[Line]) -> String {
    let mut text = String::new();
    for (index, row) in rows(lines.to_vec()).iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        for (i, line) in row.iter().enumerate() {
            if i > 0 {
                let previous = &row[i - 1].rect;
                let gap = line.rect.left() - previous.right();
                let height = previous.height().max(line.rect.height());
                text.push(if gap > height * 2.0 { '\t' } else { ' ' });
            }
            text.push_str(line.text.trim());
        }
    }
    text
}

/// 去掉两个汉字（以及汉字与中文标点）之间的空格：Windows 把每个汉字当作一个单词，单词之间用空格连接。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn remove_cjk_spaces(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    for (index, &c) in chars.iter().enumerate() {
        if c == ' ' {
            let before = result.chars().next_back();
            let after = chars[index + 1..].iter().find(|c| **c != ' ');
            if before.is_some_and(is_cjk) && after.is_some_and(|c| is_cjk(*c)) {
                continue;
            }
        }
        result.push(c);
    }
    result
}

/// 汉字、假名、谚文与全角标点（这些字符之间本来就不用空格）。
#[cfg_attr(not(windows), allow(dead_code))]
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2FDF      // 部首
        | 0x3000..=0x303F    // 中文标点
        | 0x3040..=0x30FF    // 假名
        | 0x3400..=0x4DBF    // 扩展 A
        | 0x4E00..=0x9FFF    // 基本汉字
        | 0xAC00..=0xD7AF    // 谚文
        | 0xF900..=0xFAFF    // 兼容汉字
        | 0xFF00..=0xFFEF    // 全角符号
        | 0x20000..=0x3134F  // 扩展 B–G
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn line(text: &str, x: f32, y: f32, w: f32, h: f32) -> Line {
        Line {
            text: text.to_owned(),
            rect: Rect::from_min_size(pos2(x, y), vec2(w, h)),
        }
    }

    #[test]
    fn tiles_cover_the_image_without_gaps() {
        assert_eq!(
            tiles(800, 1000, 100),
            vec![Tile {
                start: 0,
                end: 800,
                own_start: 0,
                own_end: 800
            }]
        );
        let parts = tiles(2500, 1000, 100);
        assert_eq!(parts.len(), 3);
        assert_eq!((parts[0].start, parts[0].end), (0, 1000));
        assert_eq!((parts[1].start, parts[1].end), (900, 1900));
        // 最后一块保持完整的高度，贴着底边。
        assert_eq!((parts[2].start, parts[2].end), (1500, 2500));
        // 各块“自己的”范围首尾相接，并且都在这一块里面。
        assert_eq!(parts[0].own_start, 0);
        assert_eq!(parts.last().unwrap().own_end, 2500);
        for pair in parts.windows(2) {
            assert_eq!(pair[0].own_end, pair[1].own_start);
        }
        for part in &parts {
            assert!(part.start <= part.own_start && part.own_end <= part.end);
        }
        // 重叠超过一半时按一半算，不会死循环。
        assert!(tiles(5000, 100, 1000).len() > 1);
    }

    #[test]
    fn tiled_recognition_keeps_each_line_once() {
        // 分成 4 块：每 20 像素一行字，引擎只“认出”完整落在这一块里的行，被切断的行认成乱码。
        let image = RgbaImage::new(100, 1000);
        let parts = tiles(1000, 300, 60);
        assert_eq!(parts.len(), 4);
        let mut index = 0;
        let lines = recognize_tiled(&image, 300, 60, |tile| {
            let part = parts[index];
            index += 1;
            assert_eq!(tile.height(), part.end - part.start);
            let mut result = Vec::new();
            for row in 0..50u32 {
                let (top, bottom) = (row * 20 + 4, row * 20 + 18);
                if bottom <= part.start || top >= part.end {
                    continue;
                }
                let top = top.max(part.start) - part.start;
                let bottom = bottom.min(part.end) - part.start;
                let complete = bottom - top == 14;
                let text = if complete {
                    format!("第{row}行")
                } else {
                    "乱码".to_owned()
                };
                result.push(line(&text, 5.0, top as f32, 80.0, (bottom - top) as f32));
            }
            Ok(result)
        })
        .unwrap();
        let texts: Vec<_> = lines.iter().map(|line| line.text.as_str()).collect();
        let expected: Vec<_> = (0..50).map(|row| format!("第{row}行")).collect();
        assert_eq!(texts, expected);
        // 坐标换算回整张图片。
        assert_eq!(lines[30].rect.top(), 604.0);
    }

    #[test]
    fn groups_lines_into_rows() {
        let lines = vec![
            line("第二行", 10.0, 40.0, 60.0, 20.0),
            line("右边", 200.0, 12.0, 40.0, 18.0),
            line("左边", 10.0, 10.0, 60.0, 20.0),
            line("   ", 10.0, 70.0, 60.0, 20.0),
        ];
        let ordered = reading_order(lines.clone());
        let texts: Vec<_> = ordered.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["左边", "右边", "第二行", "   "]);
        // “左边”与“右边”隔了 130 像素（超过两个字高），是两列。
        assert_eq!(to_text(&lines[..3]), "左边\t右边\n第二行");
        let near = [
            line("Hello", 10.0, 10.0, 50.0, 20.0),
            line("world", 70.0, 10.0, 50.0, 20.0),
        ];
        assert_eq!(to_text(&near), "Hello world");
        assert_eq!(to_text(&[]), "");
    }

    #[test]
    fn removes_spaces_between_cjk_characters() {
        assert_eq!(remove_cjk_spaces("你 好 ， 世 界"), "你好，世界");
        assert_eq!(remove_cjk_spaces("打开 GitHub 仓库"), "打开 GitHub 仓库");
        assert_eq!(remove_cjk_spaces("hello world"), "hello world");
        assert_eq!(remove_cjk_spaces("版本 0.1.5 发布"), "版本 0.1.5 发布");
        assert_eq!(remove_cjk_spaces("中  文"), "中文");
    }
}
