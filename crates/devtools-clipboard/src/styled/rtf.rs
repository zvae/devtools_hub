//! RTF 解析：只处理显示需要的控制字（字体、颜色、粗体、斜体、Unicode 与常见符号）。

use super::{is_monospace_font, Output, Style, StyledRun};

/// 当前组里的文字去向。
#[derive(Clone, Copy, PartialEq)]
enum Destination {
    Text,
    FontTable,
    ColorTable,
    Skip,
}

/// RTF 组的状态：进入 `{` 时压栈，`}` 时恢复。
#[derive(Clone, Copy)]
struct RtfGroup {
    destination: Destination,
    color: usize,
    bold: bool,
    italic: bool,
    font: i32,
    /// `\ucN`：`\uN` 之后跟着几个替代字符要跳过。
    unicode_skip: usize,
}

/// 不显示内容的 RTF 目标。
const SKIP_DESTINATIONS: [&str; 18] = [
    "stylesheet",
    "info",
    "pict",
    "object",
    "header",
    "headerl",
    "headerr",
    "footer",
    "footerl",
    "footerr",
    "footnote",
    "fldinst",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "generator",
    "themedata",
    "latentstyles",
];

/// 解析 RTF：按组（`{}`）维护样式栈，读出字体表、颜色表与正文。
pub(super) fn rtf_runs(data: &[u8], max_chars: usize) -> Option<Vec<StyledRun>> {
    if !data.starts_with(b"{\\rtf") {
        return None;
    }
    let mut out = Output::new(max_chars);
    // 字体号 → 是否等宽；颜色表（下标 0 通常是“自动”）。
    let mut fonts: Vec<(i32, bool)> = Vec::new();
    let mut colors: Vec<Option<[u8; 3]>> = Vec::new();
    // 正在读的字体表项与颜色表项。
    let (mut font_number, mut font_modern, mut font_name) = (0, false, String::new());
    let mut color: [Option<u8>; 3] = [None; 3];

    let mut group = RtfGroup {
        destination: Destination::Text,
        color: 0,
        bold: false,
        italic: false,
        font: 0,
        unicode_skip: 1,
    };
    let mut stack: Vec<RtfGroup> = Vec::new();
    // 刚进入一个组：组里第一个控制字可能是目标名。
    let mut group_start = false;
    let mut skip_chars = 0usize;
    let mut text = Vec::<u8>::new();
    let mut i = 0;

    let style = |group: &RtfGroup, fonts: &[(i32, bool)], colors: &[Option<[u8; 3]>]| Style {
        color: colors.get(group.color).copied().flatten(),
        bold: group.bold,
        italic: group.italic,
        monospace: fonts
            .iter()
            .find(|(number, _)| *number == group.font)
            .is_some_and(|(_, mono)| *mono),
    };
    // 把攒下的字节作为 UTF-8 输出（有的程序直接写 UTF-8，按 Latin-1 解读会乱码）。
    macro_rules! flush {
        () => {
            if !text.is_empty() {
                let chunk = String::from_utf8_lossy(&text).into_owned();
                text.clear();
                match group.destination {
                    Destination::Text => out.push(&chunk, style(&group, &fonts, &colors)),
                    Destination::FontTable => font_name.push_str(&chunk),
                    _ => {}
                }
            }
        };
    }
    let emit_char = |ch: char,
                     group: &RtfGroup,
                     out: &mut Output,
                     font_name: &mut String,
                     fonts: &[(i32, bool)],
                     colors: &[Option<[u8; 3]>]| {
        match group.destination {
            Destination::Text => {
                let mut buf = [0; 4];
                out.push(ch.encode_utf8(&mut buf), style(group, fonts, colors));
            }
            Destination::FontTable => font_name.push(ch),
            _ => {}
        }
    };

    while i < data.len() && !out.full() {
        let byte = data[i];
        match byte {
            b'{' => {
                flush!();
                stack.push(group);
                group_start = true;
                i += 1;
                continue;
            }
            b'}' => {
                flush!();
                if group.destination == Destination::FontTable && !font_name.is_empty() {
                    let name = std::mem::take(&mut font_name);
                    fonts.push((
                        font_number,
                        font_modern || is_monospace_font(name.trim_end_matches(';')),
                    ));
                }
                match stack.pop() {
                    Some(parent) => group = parent,
                    None => break,
                }
                group_start = false;
                i += 1;
                continue;
            }
            b'\\' => {
                flush!();
                let Some(&next) = data.get(i + 1) else {
                    break;
                };
                if next.is_ascii_alphabetic() {
                    // 控制字：字母、可选的带符号数字参数、可选的一个空格分隔符。
                    let start = i + 1;
                    let mut end = start;
                    while end < data.len() && data[end].is_ascii_alphabetic() {
                        end += 1;
                    }
                    let word = std::str::from_utf8(&data[start..end]).unwrap_or("");
                    let param_start = end;
                    if end < data.len() && data[end] == b'-' {
                        end += 1;
                    }
                    while end < data.len() && data[end].is_ascii_digit() {
                        end += 1;
                    }
                    let param: Option<i32> = std::str::from_utf8(&data[param_start..end])
                        .ok()
                        .and_then(|p| p.parse().ok());
                    if end < data.len() && data[end] == b' ' {
                        end += 1;
                    }
                    i = end;
                    let first = std::mem::take(&mut group_start);
                    if first && SKIP_DESTINATIONS.contains(&word) {
                        group.destination = Destination::Skip;
                        continue;
                    }
                    if group.destination == Destination::Skip {
                        continue;
                    }
                    if let Some(ch) = symbol(word) {
                        emit_char(ch, &group, &mut out, &mut font_name, &fonts, &colors);
                        continue;
                    }
                    match word {
                        "fonttbl" => group.destination = Destination::FontTable,
                        "colortbl" => group.destination = Destination::ColorTable,
                        "f" if group.destination == Destination::FontTable => {
                            font_number = param.unwrap_or(0);
                            font_modern = false;
                            font_name.clear();
                        }
                        "fmodern" => font_modern = true,
                        "red" | "green" | "blue"
                            if group.destination == Destination::ColorTable =>
                        {
                            let index = ["red", "green", "blue"]
                                .iter()
                                .position(|c| *c == word)
                                .unwrap_or(0);
                            color[index] = Some(param.unwrap_or(0).clamp(0, 255) as u8);
                        }
                        "f" => group.font = param.unwrap_or(0),
                        "cf" => group.color = param.unwrap_or(0).max(0) as usize,
                        "b" => group.bold = param != Some(0),
                        "i" => group.italic = param != Some(0),
                        "plain" => {
                            group.bold = false;
                            group.italic = false;
                            group.color = 0;
                        }
                        "uc" => group.unicode_skip = param.unwrap_or(1).max(0) as usize,
                        "u" => {
                            let code = param.unwrap_or(0);
                            let code = if code < 0 { code + 65536 } else { code } as u32;
                            if let Some(ch) = char::from_u32(code) {
                                emit_char(ch, &group, &mut out, &mut font_name, &fonts, &colors);
                            }
                            skip_chars = group.unicode_skip;
                        }
                        "bin" => i += param.unwrap_or(0).max(0) as usize,
                        _ => {}
                    }
                    continue;
                }
                // 控制符号。
                i += 2;
                let first = std::mem::take(&mut group_start);
                match next {
                    b'*' if first => group.destination = Destination::Skip,
                    b'\'' => {
                        let hex = data.get(i..i + 2).and_then(|h| std::str::from_utf8(h).ok());
                        i += 2;
                        if skip_chars > 0 {
                            skip_chars -= 1;
                        } else if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok())
                        {
                            // 非 UTF-8 的单字节，按 Latin-1 解读。
                            emit_char(
                                char::from(value),
                                &group,
                                &mut out,
                                &mut font_name,
                                &fonts,
                                &colors,
                            );
                        }
                    }
                    b'\\' | b'{' | b'}' => text.push(next),
                    b'~' => text.push(b' '),
                    b'_' => text.push(b'-'),
                    b'\n' | b'\r' => {
                        emit_char('\n', &group, &mut out, &mut font_name, &fonts, &colors)
                    }
                    _ => {}
                }
                continue;
            }
            b'\r' | b'\n' => {}
            b';' if group.destination == Destination::ColorTable => {
                colors.push(match color {
                    [Some(r), Some(g), Some(b)] => Some([r, g, b]),
                    [None, None, None] => None,
                    [r, g, b] => Some([r.unwrap_or(0), g.unwrap_or(0), b.unwrap_or(0)]),
                });
                color = [None; 3];
            }
            b';' if group.destination == Destination::FontTable => {
                flush!();
                let name = std::mem::take(&mut font_name);
                fonts.push((font_number, font_modern || is_monospace_font(&name)));
                font_modern = false;
            }
            _ => {
                group_start = false;
                if skip_chars > 0 {
                    skip_chars -= 1;
                } else {
                    text.push(byte);
                }
            }
        }
        i += 1;
    }
    flush!();
    out.finish()
}

/// 表示单个字符的控制字（`\par` 换行、`\tab` 制表符、引号、破折号等）。
fn symbol(word: &str) -> Option<char> {
    Some(match word {
        "par" | "line" | "row" | "sect" | "page" => '\n',
        "tab" | "cell" => '\t',
        "emdash" => '—',
        "endash" => '–',
        "bullet" => '•',
        "lquote" => '‘',
        "rquote" => '’',
        "ldblquote" => '“',
        "rdblquote" => '”',
        _ => return None,
    })
}
