//! 从富文本（RTF / HTML）里取出代码的高亮样式。
//!
//! IDE 复制代码时会同时写入带语法高亮的富文本：JetBrains 系列写 RTF（也写 HTML），
//! VS Code 写 HTML，Xcode、终端写 RTF。PasteNow 按这些样式显示代码，从浏览器复制的同一段文字
//! 则按普通文本显示。这里也只在文字基本都是等宽字体（也就是代码）时返回样式，
//! 网页、文档里的富文本仍按纯文本显示，免得把网页的配色搬进列表。
//!
//! 只解析显示需要的部分：文字、颜色、粗体、斜体、字体是否等宽。

use crate::model::Snapshot;
use crate::uti;

/// 一段样式相同的文字。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledRun {
    pub text: String,
    /// 没有指定颜色时为 `None`（用界面的正文颜色）。
    pub color: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
}

/// 等宽字体的文字至少占这么多，才当作代码。
const MONOSPACE_SHARE: f32 = 0.8;

/// 快照里的代码样式：优先 RTF，其次 HTML；不是代码时返回 `None`。最多取 `max_chars` 个字符。
pub fn code_runs(snapshot: &Snapshot, max_chars: usize) -> Option<Vec<StyledRun>> {
    if let Some(rtf) = snapshot.first(uti::RTF) {
        return rtf_runs(rtf, max_chars);
    }
    let html = snapshot.first(uti::HTML)?;
    html_runs(&String::from_utf8_lossy(html), max_chars)
}

/// 收集输出：合并样式相同的相邻文字，统计等宽字体的比例。
#[derive(Default)]
struct Output {
    runs: Vec<StyledRun>,
    chars: usize,
    max_chars: usize,
    visible: usize,
    monospace: usize,
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Style {
    color: Option<[u8; 3]>,
    bold: bool,
    italic: bool,
    monospace: bool,
}

impl Output {
    fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            ..Self::default()
        }
    }

    fn full(&self) -> bool {
        self.chars >= self.max_chars
    }

    fn push(&mut self, text: &str, style: Style) {
        for ch in text.chars() {
            if self.full() {
                return;
            }
            self.chars += 1;
            if !ch.is_whitespace() {
                self.visible += 1;
                if style.monospace {
                    self.monospace += 1;
                }
            }
            match self.runs.last_mut() {
                Some(run)
                    if run.color == style.color
                        && run.bold == style.bold
                        && run.italic == style.italic =>
                {
                    run.text.push(ch);
                }
                _ => self.runs.push(StyledRun {
                    text: ch.to_string(),
                    color: style.color,
                    bold: style.bold,
                    italic: style.italic,
                }),
            }
        }
    }

    fn ends_with_newline(&self) -> bool {
        self.runs.last().is_none_or(|run| run.text.ends_with('\n'))
    }

    /// 去掉末尾的换行与空白；不是代码时返回 `None`。
    fn finish(mut self) -> Option<Vec<StyledRun>> {
        if self.visible == 0 || (self.monospace as f32) < self.visible as f32 * MONOSPACE_SHARE {
            return None;
        }
        while let Some(run) = self.runs.last_mut() {
            let trimmed = run.text.trim_end().len();
            run.text.truncate(trimmed);
            if !run.text.is_empty() {
                break;
            }
            self.runs.pop();
        }
        Some(self.runs)
    }
}

/// 常见的等宽字体（RTF 字体表里的 `\fmodern` 也算）。
fn is_monospace_font(name: &str) -> bool {
    const NAMES: [&str; 16] = [
        "mono",
        "menlo",
        "monaco",
        "courier",
        "consolas",
        "fira code",
        "firacode",
        "source code",
        "sourcecode",
        "cascadia",
        "hack",
        "inconsolata",
        "iosevka",
        "sf mono",
        "sfmono",
        "lucida console",
    ];
    let name = name.to_ascii_lowercase();
    NAMES.iter().any(|mono| name.contains(mono))
}

// ---------------------------------------------------------------- RTF

#[derive(Clone, Copy, PartialEq)]
enum Destination {
    Text,
    FontTable,
    ColorTable,
    Skip,
}

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

fn rtf_runs(data: &[u8], max_chars: usize) -> Option<Vec<StyledRun>> {
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
                        "par" | "line" | "row" | "sect" | "page" => {
                            emit_char('\n', &group, &mut out, &mut font_name, &fonts, &colors)
                        }
                        "tab" | "cell" => {
                            emit_char('\t', &group, &mut out, &mut font_name, &fonts, &colors)
                        }
                        "emdash" => {
                            emit_char('—', &group, &mut out, &mut font_name, &fonts, &colors)
                        }
                        "endash" => {
                            emit_char('–', &group, &mut out, &mut font_name, &fonts, &colors)
                        }
                        "bullet" => {
                            emit_char('•', &group, &mut out, &mut font_name, &fonts, &colors)
                        }
                        "lquote" => {
                            emit_char('‘', &group, &mut out, &mut font_name, &fonts, &colors)
                        }
                        "rquote" => {
                            emit_char('’', &group, &mut out, &mut font_name, &fonts, &colors)
                        }
                        "ldblquote" => {
                            emit_char('“', &group, &mut out, &mut font_name, &fonts, &colors)
                        }
                        "rdblquote" => {
                            emit_char('”', &group, &mut out, &mut font_name, &fonts, &colors)
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

// ---------------------------------------------------------------- HTML

#[derive(Clone, Copy, Default)]
struct HtmlFrame {
    style: Style,
    /// 保留空白（`<pre>`、`white-space: pre`）。
    pre: bool,
}

/// 只需要知道开始、结束的块级元素：前后换行。
const BLOCKS: [&str; 18] = [
    "div",
    "p",
    "pre",
    "li",
    "tr",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "table",
    "ul",
    "ol",
    "section",
    "article",
    "header",
];

const VOID: [&str; 11] = [
    "br", "img", "meta", "link", "hr", "input", "wbr", "col", "area", "base", "source",
];

/// 里面的内容不显示。
const HIDDEN: [&str; 5] = ["style", "script", "head", "title", "template"];

fn html_runs(html: &str, max_chars: usize) -> Option<Vec<StyledRun>> {
    let mut out = Output::new(max_chars);
    let mut stack: Vec<(String, HtmlFrame)> = Vec::new();
    let mut rest = html;
    // 不保留空白时，上一个输出的字符是不是空白（连续空白只留一个）。
    let mut last_space = true;

    let current = |stack: &[(String, HtmlFrame)]| stack.last().map(|(_, f)| *f).unwrap_or_default();
    let newline = |out: &mut Output, last_space: &mut bool, frame: HtmlFrame| {
        if !out.ends_with_newline() {
            out.push("\n", frame.style);
        }
        *last_space = true;
    };

    while !rest.is_empty() && !out.full() {
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |end| &rest[end + 1..]);
            continue;
        }
        if let Some(after) = rest.strip_prefix('<') {
            let Some(end) = after.find('>') else {
                break;
            };
            let tag = &after[..end];
            rest = &after[end + 1..];
            if let Some(name) = tag.strip_prefix('/') {
                let name = name.trim().to_ascii_lowercase();
                if let Some(index) = stack.iter().rposition(|(open, _)| *open == name) {
                    stack.truncate(index);
                }
                if BLOCKS.contains(&name.as_str()) {
                    newline(&mut out, &mut last_space, current(&stack));
                }
                continue;
            }
            let (name, attrs) = tag
                .split_once(|c: char| c.is_ascii_whitespace())
                .unwrap_or((tag, ""));
            let name = name.trim_end_matches('/').to_ascii_lowercase();
            if HIDDEN.contains(&name.as_str()) {
                // 跳到对应的结束标签之后（标签名不区分大小写，转小写不改变字节位置）。
                let source = rest;
                let close = format!("</{name}");
                rest = source
                    .to_ascii_lowercase()
                    .find(&close)
                    .and_then(|start| {
                        let end = source[start..].find('>')?;
                        Some(&source[start + end + 1..])
                    })
                    .unwrap_or("");
                continue;
            }
            let parent = current(&stack);
            if name == "br" {
                out.push("\n", parent.style);
                last_space = true;
                continue;
            }
            if BLOCKS.contains(&name.as_str()) {
                newline(&mut out, &mut last_space, parent);
            }
            if VOID.contains(&name.as_str()) || tag.ends_with('/') {
                continue;
            }
            let mut frame = parent;
            match name.as_str() {
                "b" | "strong" => frame.style.bold = true,
                "i" | "em" => frame.style.italic = true,
                "pre" => {
                    frame.style.monospace = true;
                    frame.pre = true;
                }
                "code" | "tt" | "kbd" | "samp" => frame.style.monospace = true,
                _ => {}
            }
            for (key, value) in attributes(attrs) {
                match key.as_str() {
                    "style" => apply_css(&value, &mut frame),
                    "color" if name == "font" => frame.style.color = css_color(&value),
                    "face" if name == "font" => frame.style.monospace = is_monospace_font(&value),
                    _ => {}
                }
            }
            stack.push((name, frame));
            continue;
        }
        // 文字：到下一个标签为止。
        let end = rest.find('<').unwrap_or(rest.len());
        let decoded = decode_entities(&rest[..end]);
        rest = &rest[end..];
        let frame = current(&stack);
        if frame.pre {
            let text = decoded.replace("\r\n", "\n").replace('\u{a0}', " ");
            if let Some(last) = text.chars().last() {
                last_space = last.is_whitespace();
            }
            out.push(&text, frame.style);
        } else {
            let mut text = String::new();
            for ch in decoded.chars() {
                if ch.is_whitespace() && ch != '\u{a0}' {
                    if !last_space {
                        text.push(' ');
                        last_space = true;
                    }
                } else {
                    text.push(if ch == '\u{a0}' { ' ' } else { ch });
                    last_space = false;
                }
            }
            out.push(&text, frame.style);
        }
    }
    out.finish()
}

/// 标签里的属性：`name="value"`、`name='value'`、`name=value`。
fn attributes(text: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let mut rest = text.trim().trim_end_matches('/');
    while !rest.is_empty() {
        rest = rest.trim_start();
        let name_end = rest
            .find(|c: char| c == '=' || c.is_ascii_whitespace())
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        let Some(after) = rest.strip_prefix('=') else {
            if name.is_empty() {
                break;
            }
            attrs.push((name, String::new()));
            continue;
        };
        let after = after.trim_start();
        let (value, remaining) = match after.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let body = &after[1..];
                let end = body.find(quote).unwrap_or(body.len());
                (&body[..end], body.get(end + 1..).unwrap_or(""))
            }
            _ => {
                let end = after
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        attrs.push((name, decode_entities(value)));
        rest = remaining;
    }
    attrs
}

fn apply_css(css: &str, frame: &mut HtmlFrame) {
    for declaration in css.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_end_matches("!important").trim();
        match property.trim().to_ascii_lowercase().as_str() {
            "color" => frame.style.color = css_color(value).or(frame.style.color),
            "font-weight" => {
                frame.style.bold = matches!(value, "bold" | "bolder")
                    || value.parse::<u32>().is_ok_and(|weight| weight >= 600);
            }
            "font-style" => frame.style.italic = matches!(value, "italic" | "oblique"),
            "font-family" => frame.style.monospace = is_monospace_font(value),
            "white-space" => {
                frame.pre = matches!(value, "pre" | "pre-wrap" | "pre-line" | "break-spaces");
            }
            _ => {}
        }
    }
}

/// `#rgb`、`#rrggbb`、`#rrggbbaa`、`rgb(r, g, b)`、`rgba(r, g, b, a)` 与几个常见的颜色名。
fn css_color(value: &str) -> Option<[u8; 3]> {
    let value = value.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
        let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            3 | 4 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
            6 | 8 => Some([pair(0)?, pair(2)?, pair(4)?]),
            _ => None,
        };
    }
    if let Some(args) = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))
    {
        let mut parts = args
            .trim_end_matches(')')
            .split(|c: char| c == ',' || c.is_ascii_whitespace() || c == '/')
            .filter(|part| !part.is_empty())
            .map(|part| {
                part.parse::<f32>()
                    .ok()
                    .map(|v| v.round().clamp(0.0, 255.0) as u8)
            });
        return Some([parts.next()??, parts.next()??, parts.next()??]);
    }
    Some(match value.as_str() {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "blue" => [0, 0, 255],
        "gray" | "grey" => [128, 128, 128],
        "orange" => [255, 165, 0],
        "purple" => [128, 0, 128],
        _ => return None,
    })
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let decoded = rest.find(';').filter(|&end| end <= 10).and_then(|end| {
            let entity = &rest[1..end];
            let ch = match entity {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                    .and_then(char::from_u32),
            };
            ch.map(|ch| (ch, end))
        });
        match decoded {
            Some((ch, end)) => {
                out.push(ch);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(runs: &[StyledRun]) -> String {
        runs.iter().map(|run| run.text.as_str()).collect()
    }

    #[test]
    fn reads_jetbrains_rtf() {
        let rtf = br"{\rtf1\ansi\deff0{\fonttbl{\f0\fnil JetBrains Mono;}}
{\colortbl;\red169\green183\blue198;\red43\green43\blue43;\red204\green120\blue50;\red152\green118\blue170;}
\f0\fs26\cb2\cf1\highlight2 \cf3 SELECT\cf1  r.* \cf3 FROM\cf1  t_order r\par
\cf3 WHERE\cf1  r.\cf4 id\cf1  = 1\par
}";
        let runs = rtf_runs(rtf, 1000).expect("等宽字体的 RTF 是代码");
        assert_eq!(plain(&runs), "SELECT r.* FROM t_order r\nWHERE r.id = 1");
        assert_eq!(runs[0].text, "SELECT");
        assert_eq!(runs[0].color, Some([204, 120, 50]));
        assert_eq!(runs[1].color, Some([169, 183, 198]));
        let id = runs.iter().find(|run| run.text == "id").unwrap();
        assert_eq!(id.color, Some([152, 118, 170]));
    }

    #[test]
    fn reads_cocoa_rtf_with_unicode_and_skips_destinations() {
        let rtf = br"{\rtf1\ansi\ansicpg936\cocoartf2822
\cocoatextscaling0\cocoaplatform0{\fonttbl\f0\fnil\fcharset0 Menlo-Bold;\f1\fnil\fcharset0 Menlo-Regular;}
{\colortbl;\red255\green255\blue255;\red155\green35\blue147;}
{\*\expandedcolortbl;;\csgenericrgb\c60759\c13753\c57628;}
\pard\tx593\pardeftab593\partightenfactor0

\f0\b\fs24 \cf2 let\f1\b0 \cf0  name = \cf2 \uc0\u20013 \u25991 \cf0 \
}";
        let runs = rtf_runs(rtf, 1000).unwrap();
        assert_eq!(plain(&runs), "let name = 中文");
        assert!(runs[0].bold);
        assert_eq!(runs[0].color, Some([155, 35, 147]));
        assert_eq!(runs[1].color, None, "\\cf0 是自动颜色");
    }

    #[test]
    fn proportional_rtf_is_not_code() {
        let rtf = br"{\rtf1\ansi{\fonttbl\f0\fswiss Helvetica;}{\colortbl;\red255\green0\blue0;}\f0\cf1 Hello world\par}";
        assert_eq!(rtf_runs(rtf, 1000), None);
    }

    #[test]
    fn reads_vscode_html() {
        let html = "<meta charset='utf-8'><div style=\"color: #cccccc;background-color: #1f1f1f;font-family: Menlo, Monaco, 'Courier New', monospace;font-weight: normal;font-size: 12px;line-height: 18px;white-space: pre;\"><div><span style=\"color: #569cd6;\">fn</span><span style=\"color: #cccccc;\"> </span><span style=\"color: #dcdcaa;\">main</span><span style=\"color: #cccccc;\">() {</span></div><div><span style=\"color: #cccccc;\">    </span><span style=\"color: #c586c0;\">let</span><span style=\"color: #cccccc;\"> a = </span><span style=\"color: #ce9178;\">&quot;x &amp; y&quot;</span><span style=\"color: #cccccc;\">;</span></div><div><br></div><div><span style=\"color: #cccccc;\">}</span></div></div>";
        let runs = html_runs(html, 1000).expect("等宽字体的 HTML 是代码");
        assert_eq!(plain(&runs), "fn main() {\n    let a = \"x & y\";\n\n}");
        assert_eq!(runs[0].color, Some([0x56, 0x9c, 0xd6]));
        let string = runs.iter().find(|run| run.text.contains("x & y")).unwrap();
        assert_eq!(string.color, Some([0xce, 0x91, 0x78]));
    }

    #[test]
    fn reads_jetbrains_html() {
        let html = "<html><head><style type=\"text/css\">.s0 { color: #cc7832;}</style></head><body><pre style=\"background-color:#2b2b2b;color:#a9b7c6;font-family:'JetBrains Mono',monospace;font-size:9.8pt;\"><span style=\"color:#cc7832;font-weight:bold;\">SELECT</span> *\n<span style=\"color:#cc7832;\">FROM</span> t</pre></body></html>";
        let runs = html_runs(html, 1000).unwrap();
        assert_eq!(plain(&runs), "SELECT *\nFROM t");
        assert!(runs[0].bold);
        assert_eq!(runs[1].color, Some([0xa9, 0xb7, 0xc6]));
    }

    #[test]
    fn web_html_is_not_code() {
        let html = "<meta charset='utf-8'><span style=\"color: rgb(32, 33, 36); font-family: arial, sans-serif;\">and r.create_time &gt; NOW()</span>";
        assert_eq!(html_runs(html, 1000), None);
        // 网页里的行内代码只占一小部分，也不算。
        let html = "<p style=\"font-family: Helvetica\">运行 <code>cargo build</code> 之后再打开应用，然后检查输出目录</p>";
        assert_eq!(html_runs(html, 1000), None);
    }

    #[test]
    fn stops_at_max_chars() {
        let html = "<pre>abcdefghij</pre>";
        assert_eq!(plain(&html_runs(html, 4).unwrap()), "abcd");
    }
}
