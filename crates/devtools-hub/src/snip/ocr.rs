//! 文字识别（OCR）中与平台无关的部分（方案见 `docs/微信滚动截图与文字提取实现分析.md` 的 4.2 节）：
//!
//! 识别本身交给系统（macOS 的 Vision、Windows 的 `Windows.Media.Ocr`，在 `platform::ocr` 里），这里负责：
//!
//! - 分块：长截图比屏幕高得多，整张交给系统时会被缩小到看不清，按高度切成有重叠的几块分别识别，
//!   每一行只保留在它中心所在的那一块里的结果（被切断的半行在另一块里是完整的）；
//! - 阅读顺序：把识别出的文字行按行（上下位置重叠）分组，组内从左到右；
//! - 合并成文本：行与行之间换行，同一行的几段用空格隔开（离得远的用制表符）；
//! - 去掉 Windows 在汉字之间插入的空格（它按“单词”识别，每个汉字是一个单词）。

use std::ops::Range;

use anyhow::Result;
use egui::{vec2, Rect};
use image::RgbaImage;

/// 识别出的一行文字。
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    /// 在图片中的位置（像素，左上角为原点）。
    pub rect: Rect,
    /// 每个字符在图片中的位置，与 `text` 的字符一一对应（左边的字符在前）。
    /// 平台给不出时按 [`estimate_chars`] 平铺在 [`Self::rect`] 里。
    pub chars: Vec<Rect>,
}

impl Line {
    /// 一行文字：`chars` 留给 [`fill_chars`] 补。
    pub fn new(text: impl Into<String>, rect: Rect) -> Self {
        Self {
            text: text.into(),
            rect,
            chars: Vec::new(),
        }
    }
}

/// 按字符的相对宽度把 [`Line::rect`] 切开：界面里在图片上拖过一行时，据此知道拖到了第几个字。
///
/// 汉字、假名、全角标点等宽，西文按字宽大致估一个比例——只用于没给出字符位置的平台
/// （Windows 的识别结果以单词为单位）；差一点不影响选中，右边标亮的文字是精确的。
pub fn estimate_chars(text: &str, rect: Rect) -> Vec<Rect> {
    let widths: Vec<f32> = text.chars().map(char_width).collect();
    let total: f32 = widths.iter().sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let mut x = rect.left();
    widths
        .into_iter()
        .map(|width| {
            let next = x + width / total * rect.width();
            let cell =
                Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(next, rect.bottom()));
            x = next;
            cell
        })
        .collect()
}

/// 字符的相对宽度（一个汉字算 1）。
fn char_width(c: char) -> f32 {
    if c.is_whitespace() {
        0.3
    } else if (c as u32) >= 0x2E80 {
        // 汉字、假名、谚文与全角标点。
        1.0
    } else if "iljtfr.,;:'!|".contains(c) {
        0.3
    } else if c.is_ascii_uppercase() || c.is_ascii_digit() {
        0.6
    } else if c.is_ascii() {
        0.5
    } else {
        0.8
    }
}

/// 补齐空的字符位置，并把每个字符的位置整理成这一行里的横向格子（高度取整行，x 单调不减）。
///
/// 平台的坐标有时会略微超出这一行（上下伸出一点、相邻字符重叠一点），整理之后画高亮、判断拖动位置都简单。
pub fn fill_chars(lines: &mut [Line]) {
    for line in lines.iter_mut() {
        if line.chars.len() != line.text.chars().count() {
            line.chars = estimate_chars(&line.text, line.rect);
        }
        let (left, right) = (line.rect.left(), line.rect.right());
        let mut previous = left;
        for cell in &mut line.chars {
            let start = cell.min.x.max(previous).clamp(left, right);
            let end = cell.max.x.max(start).clamp(left, right);
            *cell = Rect::from_min_max(
                egui::pos2(start, line.rect.top()),
                egui::pos2(end, line.rect.bottom()),
            );
            previous = end;
        }
    }
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
                        chars: line
                            .chars
                            .iter()
                            .map(|cell| cell.translate(offset))
                            .collect(),
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
    let mut lines = reading_order(lines);
    fill_chars(&mut lines);
    Ok(lines)
}

/// 按阅读顺序排列：上下位置重叠超过矮的那一段一半的文字行算同一行，同一行内从左到右。
pub fn reading_order(lines: Vec<Line>) -> Vec<Line> {
    rows_of(&lines)
        .into_iter()
        .flatten()
        .map(|index| lines[index].clone())
        .collect()
}

/// 按阅读顺序把文字行分成一行行：元素是 `lines` 的下标，每行内从左到右。
pub fn rows_of(lines: &[Line]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|&a, &b| lines[a].rect.top().total_cmp(&lines[b].rect.top()));
    let mut rows: Vec<(Rect, Vec<usize>)> = Vec::new();
    for index in order {
        let rect = lines[index].rect;
        let same_row = rows.last().is_some_and(|(span, _)| {
            let overlap = span.bottom().min(rect.bottom()) - span.top().max(rect.top());
            overlap > span.height().min(rect.height()) * 0.5
        });
        match rows.last_mut() {
            Some((span, row)) if same_row => {
                *span = span.union(rect);
                row.push(index);
            }
            _ => rows.push((rect, vec![index])),
        }
    }
    rows.into_iter()
        .map(|(_, mut row)| {
            row.sort_by(|&a, &b| lines[a].rect.left().total_cmp(&lines[b].rect.left()));
            row
        })
        .collect()
}

/// 合并成纯文本：行与行之间换行；同一行的几段离得近时用空格隔开，离得远（超过两个字高，
/// 多半是表格的不同列或并排的两栏）时用制表符隔开，粘贴到表格里正好分列。
#[cfg_attr(not(test), allow(dead_code))]
pub fn to_text(lines: &[Line]) -> String {
    to_text_with_spans(lines).0
}

/// 同 [`to_text`]，另外给出每一行落在文本里的字节范围（与 `lines` 同序，空行是空范围）：
/// 界面上在图片里选中的行据此在右边的文字里标亮（见 `ui::ocr`）。
pub fn to_text_with_spans(lines: &[Line]) -> (String, Vec<Range<usize>>) {
    let mut text = String::new();
    let mut spans = vec![0..0; lines.len()];
    for (index, row) in rows_of(lines).iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        for (i, &line) in row.iter().enumerate() {
            if i > 0 {
                let previous = &lines[row[i - 1]].rect;
                let rect = lines[line].rect;
                let gap = rect.left() - previous.right();
                let height = previous.height().max(rect.height());
                text.push(if gap > height * 2.0 { '\t' } else { ' ' });
            }
            let start = text.len();
            text.push_str(lines[line].text.trim());
            spans[line] = start..text.len();
        }
    }
    (text, spans)
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
        Line::new(text, Rect::from_min_size(pos2(x, y), vec2(w, h)))
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
    fn spans_point_at_each_line_in_the_text() {
        let lines = vec![
            line("第二行", 10.0, 40.0, 60.0, 20.0),
            line("右边", 200.0, 12.0, 40.0, 18.0),
            line("左边", 10.0, 10.0, 60.0, 20.0),
        ];
        let (text, spans) = to_text_with_spans(&lines);
        assert_eq!(text, "左边\t右边\n第二行");
        // 范围按 `lines` 的下标给出，指向这一行在文本里的位置（同一行的分隔符不算在内）。
        assert_eq!(&text[spans[2].clone()], "左边");
        assert_eq!(&text[spans[1].clone()], "右边");
        assert_eq!(&text[spans[0].clone()], "第二行");
        // 与 `to_text` 是同一份文本。
        assert_eq!(text, to_text(&lines));
    }

    #[test]
    fn estimates_a_cell_for_each_character() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(100.0, 20.0));
        let cells = estimate_chars("中文ab", rect);
        assert_eq!(cells.len(), 4);
        // 从左到右首尾相接，都落在这一行里。
        assert_eq!(cells[0].left(), 10.0);
        assert!((cells.last().unwrap().right() - 110.0).abs() < 0.01);
        for pair in cells.windows(2) {
            assert_eq!(pair[0].right(), pair[1].left());
        }
        // 一个汉字比一个大写字母宽。
        assert!(cells[0].width() > cells[2].width());
        assert_eq!(cells[0].top(), 20.0);
        assert_eq!(cells[0].bottom(), 40.0);
        // 补位置：字符数对不上时整行平铺；平台给的格子会整理成一行里的横向格子。
        let mut lines = vec![
            line("中文", 10.0, 20.0, 100.0, 20.0),
            Line {
                text: "ab".into(),
                rect,
                chars: vec![
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(50.0, 100.0)),
                    Rect::from_min_max(pos2(40.0, 0.0), pos2(140.0, 100.0)),
                ],
            },
        ];
        fill_chars(&mut lines);
        assert_eq!(lines[0].chars.len(), 2);
        assert_eq!(
            (lines[1].chars[0].left(), lines[1].chars[0].right()),
            (10.0, 50.0)
        );
        assert_eq!(
            (lines[1].chars[1].left(), lines[1].chars[1].right()),
            (50.0, 110.0)
        );
        assert_eq!(lines[1].chars[1].height(), 20.0);
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
