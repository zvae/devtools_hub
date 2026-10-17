//! HTML 解析：不建 DOM，顺序扫描标签与文字，按元素与内联样式确定每段文字的样式。

use super::{is_monospace_font, Output, Style, StyledRun};

/// 当前所在元素的样式（每个打开的标签一层）。
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

/// 没有结束标签的元素。
const VOID: [&str; 11] = [
    "br", "img", "meta", "link", "hr", "input", "wbr", "col", "area", "base", "source",
];

/// 里面的内容不显示。
const HIDDEN: [&str; 5] = ["style", "script", "head", "title", "template"];

/// 解析 HTML：按标签维护样式栈，读取 `style` 属性与 `<b>`、`<pre>`、`<code>` 等标签的样式。
pub(super) fn html_runs(html: &str, max_chars: usize) -> Option<Vec<StyledRun>> {
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

/// 读取 `style` 属性里与显示相关的 CSS（颜色、粗细、斜体、字体、空白处理）。
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

/// 解码 HTML 实体：`&lt;`、`&amp;`、`&nbsp;`、`&#123;`、`&#x1F600;` 等；无法识别的保持原样。
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
