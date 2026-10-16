//! JSON 文本处理：校验、格式化、压缩、转义、去除注释、路径过滤。
//!
//! 与 uTools「JSON 编辑器」插件一样直接处理文本（插件用 `formatText` 加 Monaco 的
//! `editor.action.formatDocument` 格式化），而不是解析成值再序列化回去：
//! 数字字面量（`2.0`、超出 f64 精度的大整数）、键的顺序和重复的键都原样保留；
//! 允许 `//` 与 `/* */` 注释，格式化时保留，“去除注释”时删掉。

use std::borrow::Cow;
use std::fmt;

/// 词法单元的种类。
///
/// 词法分析是宽松的：任何输入都能完整切分（语法高亮也用它），是否合法由 [`parse`] 判断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// `{` `}` `[` `]` `:` `,`
    Punct(u8),
    String,
    Number,
    /// 由字母组成的单词：`true` / `false` / `null`，以及其他不合法的单词。
    Word,
    LineComment,
    BlockComment,
    Whitespace,
    /// 无法识别的字符。
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// 字节偏移，总是落在字符边界上。
    pub start: usize,
    pub end: usize,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }

    /// 空白和注释：不影响 JSON 的结构。
    pub fn is_trivia(&self) -> bool {
        matches!(
            self.kind,
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment
        )
    }
}

/// 把文本切分成词法单元，所有单元首尾相接、覆盖全部输入。
pub fn tokenize(src: &str) -> Vec<Token> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let kind = match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => {
                while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                TokenKind::Whitespace
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                i += 1;
                TokenKind::Punct(bytes[start])
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        // 字符串不能跨行：在行尾结束，由 parse 报告“字符串没有结束”。
                        b'\n' | b'\r' => break,
                        _ => i += 1,
                    }
                }
                // 反斜杠在末尾时会越界；其余情况都停在 ASCII 字符上，是合法的字符边界。
                i = i.min(bytes.len());
                TokenKind::String
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                    i += 1;
                }
                TokenKind::LineComment
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = src[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| i + 2 + end + 2);
                TokenKind::BlockComment
            }
            b'-' | b'0'..=b'9' => {
                i += 1;
                while i < bytes.len()
                    && matches!(bytes[i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    i += 1;
                }
                TokenKind::Number
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                TokenKind::Word
            }
            _ => {
                let ch = src[i..].chars().next().unwrap_or('\0');
                i += ch.len_utf8();
                // 网页上复制的 JSON 常带不换行空格（U+00A0）或 BOM，按空白处理。
                if ch.is_whitespace() || ch == '\u{feff}' {
                    TokenKind::Whitespace
                } else {
                    TokenKind::Invalid
                }
            }
        };
        tokens.push(Token {
            kind,
            start,
            end: i,
        });
    }
    tokens
}

/// 语法错误及其位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    /// 字节偏移。
    pub offset: usize,
    /// 行号，从 1 开始。
    pub line: usize,
    /// 列号，从 1 开始，按字符计。
    pub column: usize,
    pub message: String,
}

impl JsonError {
    fn at(src: &str, offset: usize, message: impl Into<String>) -> Self {
        let before = &src[..offset];
        let line_start = before.rfind('\n').map_or(0, |index| index + 1);
        Self {
            offset,
            line: before.matches('\n').count() + 1,
            column: before[line_start..].chars().count() + 1,
            message: message.into(),
        }
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "第 {} 行第 {} 列：{}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for JsonError {}

/// 解析结果。标量与键都保存原文，输出时不会改变数字的写法。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node<'a> {
    /// 字符串（含引号）、数字、`true` / `false` / `null`。
    Scalar(Cow<'a, str>),
    Array(Vec<Node<'a>>),
    /// 键是带引号的原文。
    Object(Vec<(&'a str, Node<'a>)>),
}

/// 嵌套层数上限，防止异常输入耗尽栈空间。
const MAX_DEPTH: usize = 512;

/// 校验并解析 JSON（允许注释）。
pub fn parse(src: &str) -> Result<Node<'_>, JsonError> {
    let tokens = tokenize(src);
    for token in &tokens {
        if token.kind == TokenKind::BlockComment {
            let text = token.text(src);
            if text.len() < 4 || !text.ends_with("*/") {
                return Err(JsonError::at(src, token.start, "注释没有结束，缺少“*/”"));
            }
        }
    }
    let mut parser = Parser {
        src,
        tokens: tokens.into_iter().filter(|t| !t.is_trivia()).collect(),
        pos: 0,
        depth: 0,
    };
    if parser.tokens.is_empty() {
        return Err(JsonError::at(src, 0, "内容为空"));
    }
    let node = parser.value()?;
    if let Some(extra) = parser.next() {
        return Err(parser.error(extra.start, "JSON 已经结束，后面还有多余的内容"));
    }
    Ok(node)
}

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).copied();
        self.pos += 1;
        token
    }

    fn peek_is(&self, punct: u8) -> bool {
        self.tokens
            .get(self.pos)
            .is_some_and(|t| t.kind == TokenKind::Punct(punct))
    }

    fn error(&self, offset: usize, message: impl Into<String>) -> JsonError {
        JsonError::at(self.src, offset, message)
    }

    fn eof(&self, message: &str) -> JsonError {
        self.error(self.src.len(), message)
    }

    fn value(&mut self) -> Result<Node<'a>, JsonError> {
        let Some(token) = self.next() else {
            return Err(self.eof("内容不完整，缺少值"));
        };
        let text = token.text(self.src);
        match token.kind {
            TokenKind::Punct(b'{') => self.object(token),
            TokenKind::Punct(b'[') => self.array(token),
            TokenKind::String => {
                self.check_string(token)?;
                Ok(Node::Scalar(Cow::Borrowed(text)))
            }
            TokenKind::Number => {
                if valid_number(text) {
                    Ok(Node::Scalar(Cow::Borrowed(text)))
                } else {
                    Err(self.error(token.start, format!("无效的数字“{text}”")))
                }
            }
            TokenKind::Word if matches!(text, "true" | "false" | "null") => {
                Ok(Node::Scalar(Cow::Borrowed(text)))
            }
            TokenKind::Word => Err(self.error(
                token.start,
                format!("无法识别“{text}”：值应为字符串、数字、对象、数组、true、false 或 null（字符串需要用双引号）"),
            )),
            TokenKind::Punct(punct) => Err(self.error(
                token.start,
                format!("此处应为值，却遇到了“{}”", punct as char),
            )),
            _ => Err(self.error(token.start, format!("无法识别的字符“{text}”"))),
        }
    }

    fn enter(&mut self, open: Token) -> Result<(), JsonError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error(open.start, format!("嵌套超过 {MAX_DEPTH} 层")));
        }
        Ok(())
    }

    fn object(&mut self, open: Token) -> Result<Node<'a>, JsonError> {
        self.enter(open)?;
        let mut entries = Vec::new();
        if self.peek_is(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Node::Object(entries));
        }
        loop {
            let Some(key) = self.next() else {
                return Err(self.eof("对象没有结束，缺少“}”"));
            };
            if key.kind != TokenKind::String {
                return Err(self.error(key.start, "键必须是用双引号括起来的字符串"));
            }
            self.check_string(key)?;
            match self.next() {
                Some(colon) if colon.kind == TokenKind::Punct(b':') => {}
                Some(other) => return Err(self.error(other.start, "键后面应为“:”")),
                None => return Err(self.eof("键后面应为“:”")),
            }
            let value = self.value()?;
            entries.push((key.text(self.src), value));
            match self.next() {
                Some(t) if t.kind == TokenKind::Punct(b',') => {
                    if self.peek_is(b'}') {
                        return Err(self.error(t.start, "多余的逗号：最后一项后面不能有“,”"));
                    }
                }
                Some(t) if t.kind == TokenKind::Punct(b'}') => break,
                Some(t) => return Err(self.error(t.start, "缺少“,”或“}”")),
                None => return Err(self.eof("对象没有结束，缺少“}”")),
            }
        }
        self.depth -= 1;
        Ok(Node::Object(entries))
    }

    fn array(&mut self, open: Token) -> Result<Node<'a>, JsonError> {
        self.enter(open)?;
        let mut items = Vec::new();
        if self.peek_is(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Node::Array(items));
        }
        loop {
            items.push(self.value()?);
            match self.next() {
                Some(t) if t.kind == TokenKind::Punct(b',') => {
                    if self.peek_is(b']') {
                        return Err(self.error(t.start, "多余的逗号：最后一项后面不能有“,”"));
                    }
                }
                Some(t) if t.kind == TokenKind::Punct(b']') => break,
                Some(t) => return Err(self.error(t.start, "缺少“,”或“]”")),
                None => return Err(self.eof("数组没有结束，缺少“]”")),
            }
        }
        self.depth -= 1;
        Ok(Node::Array(items))
    }

    fn check_string(&self, token: Token) -> Result<(), JsonError> {
        let text = token.text(self.src);
        let mut chars = text.char_indices().skip(1);
        while let Some((index, ch)) = chars.next() {
            let at = token.start + index;
            match ch {
                '"' => return Ok(()),
                '\\' => match chars.next() {
                    Some((_, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't')) => {}
                    Some((_, 'u')) => {
                        let hex: String = chars.by_ref().take(4).map(|(_, c)| c).collect();
                        if hex.len() != 4 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                            return Err(self.error(at, "无效的转义：\\u 后面应为 4 位十六进制数"));
                        }
                    }
                    Some((_, other)) => {
                        return Err(self.error(at, format!("无效的转义“\\{other}”")));
                    }
                    None => break,
                },
                '\t' => return Err(self.error(at, "字符串中不能直接包含制表符，应写成 \\t")),
                ch if (ch as u32) < 0x20 => {
                    return Err(self.error(at, "字符串中不能包含控制字符"));
                }
                _ => {}
            }
        }
        Err(self.error(token.start, "字符串没有结束，缺少右引号"))
    }
}

/// `-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?`
fn valid_number(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - start
    };
    if bytes.first() == Some(&b'-') {
        i += 1;
    }
    match bytes.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => {
            digits(&mut i);
        }
        _ => return false,
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        if digits(&mut i) == 0 {
            return false;
        }
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == bytes.len()
}

/// 缩进方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Indent {
    Two,
    #[default]
    Four,
    Tab,
}

impl Indent {
    pub const ALL: [Indent; 3] = [Indent::Two, Indent::Four, Indent::Tab];

    pub fn unit(self) -> &'static str {
        match self {
            Indent::Two => "  ",
            Indent::Four => "    ",
            Indent::Tab => "\t",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Indent::Two => "2 空格",
            Indent::Four => "4 空格",
            Indent::Tab => "Tab",
        }
    }
}

/// 格式化：每个成员一行，保留注释、数字原文与键的顺序。
pub fn format(src: &str, indent: Indent) -> Result<String, JsonError> {
    parse(src)?;
    Ok(reformat(src, indent.unit()))
}

/// 按词法单元重新排版。调用前应已通过 [`parse`] 校验。
fn reformat(src: &str, indent: &str) -> String {
    let tokens = tokenize(src);
    let mut out = String::with_capacity(src.len() + src.len() / 2);
    let mut depth = 0usize;
    // 下一个单元要另起一行（刚输出了 `{`、`[`、`,` 或行注释）。
    let mut pending_newline = false;
    // 当前行以行注释结尾，后面的内容必须换行。
    let mut in_line_comment = false;
    // 原文中，上一个单元与当前单元之间有换行（用来区分行尾注释和独占一行的注释）。
    let mut newline_before = false;
    // 行内块注释后面需要一个空格再接值。
    let mut space_pending = false;

    let newline = |out: &mut String, depth: usize| {
        if !out.is_empty() {
            out.push('\n');
        }
        for _ in 0..depth {
            out.push_str(indent);
        }
    };

    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        let text = token.text(src);
        i += 1;
        match token.kind {
            TokenKind::Whitespace => {
                newline_before |= text.contains('\n');
                continue;
            }
            TokenKind::Punct(open @ (b'{' | b'[')) => {
                if pending_newline || in_line_comment {
                    newline(&mut out, depth);
                } else if space_pending {
                    out.push(' ');
                }
                out.push(open as char);
                let close = if open == b'{' { b'}' } else { b']' };
                // 空对象、空数组写在一行（中间只有空白时）。
                let next = (i..tokens.len()).find(|&j| tokens[j].kind != TokenKind::Whitespace);
                if let Some(j) = next.filter(|&j| tokens[j].kind == TokenKind::Punct(close)) {
                    out.push(close as char);
                    i = j + 1;
                    pending_newline = false;
                } else {
                    depth += 1;
                    pending_newline = true;
                }
                in_line_comment = false;
            }
            TokenKind::Punct(close @ (b'}' | b']')) => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(close as char);
                pending_newline = false;
                in_line_comment = false;
            }
            TokenKind::Punct(punct) => {
                if in_line_comment {
                    newline(&mut out, depth);
                    in_line_comment = false;
                }
                if punct == b':' {
                    out.push_str(": ");
                } else {
                    out.push(punct as char);
                    pending_newline = true;
                }
            }
            TokenKind::LineComment | TokenKind::BlockComment => {
                let line_comment = token.kind == TokenKind::LineComment;
                if out.is_empty() {
                    out.push_str(text);
                    pending_newline = true;
                } else if newline_before || in_line_comment {
                    // 独占一行的注释：保持独占一行，后面的内容另起一行。
                    newline(&mut out, depth);
                    out.push_str(text);
                    pending_newline = true;
                } else {
                    // 行尾 / 行内注释：留在当前行。
                    if !out.ends_with(' ') {
                        out.push(' ');
                    }
                    out.push_str(text);
                    space_pending = !line_comment && !pending_newline;
                    if line_comment {
                        pending_newline = true;
                    }
                }
                in_line_comment = line_comment;
                newline_before = false;
                continue;
            }
            TokenKind::String | TokenKind::Number | TokenKind::Word | TokenKind::Invalid => {
                if pending_newline || in_line_comment {
                    newline(&mut out, depth);
                } else if space_pending {
                    out.push(' ');
                }
                out.push_str(text);
                pending_newline = false;
                in_line_comment = false;
            }
        }
        space_pending = false;
        newline_before = false;
    }
    out
}

/// 压缩成一行：去掉所有空白和注释。
pub fn minify(src: &str) -> Result<String, JsonError> {
    parse(src)?;
    Ok(tokenize(src)
        .iter()
        .filter(|t| !t.is_trivia())
        .map(|t| t.text(src))
        .collect())
}

/// 压缩后再转义成字符串字面量的内容（不含两端引号），可以直接嵌进代码或另一个 JSON。
pub fn escape(src: &str) -> Result<String, JsonError> {
    let compact = minify(src)?;
    let quoted = serde_json::to_string(&compact).unwrap_or_default();
    Ok(quoted[1..quoted.len() - 1].to_string())
}

/// 去掉一层转义：`{\"a\":1}` 或 `"{\"a\":1}"` → `{"a":1}`。不是转义过的文本时返回 None。
pub fn unescape(src: &str) -> Option<String> {
    let trimmed = src.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        if let Ok(text) = serde_json::from_str::<String>(trimmed) {
            return Some(text);
        }
    }
    if !trimmed.contains('\\') {
        return None;
    }
    serde_json::from_str::<String>(&format!("\"{trimmed}\"")).ok()
}

/// 删除所有注释（不改变其他内容）。
pub fn strip_comments(src: &str) -> String {
    tokenize(src)
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::LineComment | TokenKind::BlockComment))
        .map(|t| t.text(src))
        .collect()
}

/// 打开编辑器时对传入文本的处理结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub text: String,
    /// 给用户的提示，例如“已去除转义”。
    pub note: Option<&'static str>,
}

/// 打开编辑器时自动整理传入的文本：合法 JSON 直接格式化；转义过的 JSON
/// （`"{\"a\":1}"` 或 `{\"a\":1}`）先去掉转义再格式化；都不是时保持原样，由编辑器显示错误位置。
pub fn prepare(src: &str, indent: Indent) -> Prepared {
    match parse(src) {
        Ok(Node::Scalar(scalar)) if scalar.starts_with('"') => {
            let inner = serde_json::from_str::<String>(&scalar).ok();
            if let Some(text) = inner.filter(|inner| is_container(inner)) {
                return Prepared {
                    text: format(&text, indent).unwrap_or(text),
                    note: Some("已去除一层转义"),
                };
            }
        }
        Ok(_) => {
            return Prepared {
                text: reformat(src, indent.unit()),
                note: None,
            }
        }
        Err(_) => {
            if let Some(text) = unescape(src).filter(|text| parse(text).is_ok()) {
                return Prepared {
                    text: reformat(&text, indent.unit()),
                    note: Some("已去除转义"),
                };
            }
        }
    }
    Prepared {
        text: src.to_string(),
        note: None,
    }
}

/// 是否是对象或数组。
fn is_container(src: &str) -> bool {
    matches!(parse(src), Ok(Node::Object(_) | Node::Array(_)))
}

/// 选中的文本是否像 JSON，用于决定是否在快捷面板中提供“JSON 编辑器”。
///
/// uTools 插件的匹配规则是 `/^\s*(\{[\s\S]*\}|\[[\s\S]*\])\s*$/`（最少 2 个字符）；
/// 这里同样只看首尾的括号，不要求一定合法（不合法时由编辑器标出错误位置），
/// 另外也接受转义过的 JSON 字符串。
pub fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim();
    let bracketed = (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'));
    if bracketed && trimmed.len() >= 2 {
        return true;
    }
    unescape(trimmed).is_some_and(|inner| is_container(&inner))
}

/// 把节点按指定缩进输出。
pub fn write_node(node: &Node<'_>, indent: Indent) -> String {
    let mut out = String::new();
    write_into(node, indent.unit(), 0, &mut out);
    out
}

fn write_into(node: &Node<'_>, indent: &str, depth: usize, out: &mut String) {
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        for _ in 0..depth {
            out.push_str(indent);
        }
    };
    match node {
        Node::Scalar(text) => out.push_str(text),
        Node::Array(items) if items.is_empty() => out.push_str("[]"),
        Node::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Node::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, depth + 1);
                write_into(item, indent, depth + 1, out);
            }
            newline(out, depth);
            out.push(']');
        }
        Node::Object(entries) => {
            out.push('{');
            for (index, (key, value)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, depth + 1);
                out.push_str(key);
                out.push_str(": ");
                write_into(value, indent, depth + 1, out);
            }
            newline(out, depth);
            out.push('}');
        }
    }
}

/// 路径中的一段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Key(String),
    /// 负数表示从末尾数起。
    Index(i64),
    /// `[]` / `[*]` / `.*`：对每个元素（或对象的每个值）分别取后续路径，结果组成数组。
    Each,
}

/// 解析过滤路径，语法与 uTools 插件的示例一致：`.key.subkey`、`[0][1]`，
/// 另外支持 `["含空格的键"]`、`[-1]`（倒数第一个）与 `[]`（展开数组，相当于 `.map(x => …)`）。
/// 开头的 `this` 和第一个点可以省略。
pub fn parse_path(expr: &str) -> Result<Vec<Segment>, String> {
    let expr = expr.trim();
    let expr = match expr.strip_prefix("this") {
        Some(rest) if rest.is_empty() || rest.starts_with(['.', '[']) => rest,
        _ => expr,
    };
    let chars: Vec<char> = expr.chars().collect();
    let mut segments = Vec::new();
    let mut i = 0;
    let ident = |i: &mut usize| -> String {
        let start = *i;
        while *i < chars.len()
            && !matches!(chars[*i], '.' | '[' | ']')
            && !chars[*i].is_whitespace()
        {
            *i += 1;
        }
        chars[start..*i].iter().collect()
    };
    if i < chars.len() && !matches!(chars[i], '.' | '[') {
        segments.push(Segment::Key(ident(&mut i)));
    }
    while i < chars.len() {
        match chars[i] {
            '.' => {
                i += 1;
                match chars.get(i) {
                    Some('[') => continue,
                    Some('*') => {
                        i += 1;
                        segments.push(Segment::Each);
                    }
                    _ => {
                        let key = ident(&mut i);
                        if key.is_empty() {
                            return Err("“.”后面应为键名".into());
                        }
                        segments.push(Segment::Key(key));
                    }
                }
            }
            '[' => {
                i += 1;
                while chars.get(i).is_some_and(|c| c.is_whitespace()) {
                    i += 1;
                }
                let segment = match chars.get(i) {
                    Some(']') => Segment::Each,
                    Some('*') => {
                        i += 1;
                        Segment::Each
                    }
                    Some(&quote @ ('"' | '\'')) => {
                        i += 1;
                        let mut key = String::new();
                        loop {
                            match chars.get(i) {
                                Some('\\') => {
                                    if let Some(&next) = chars.get(i + 1) {
                                        key.push(next);
                                    }
                                    i += 2;
                                }
                                Some(&c) if c == quote => {
                                    i += 1;
                                    break;
                                }
                                Some(&c) => {
                                    key.push(c);
                                    i += 1;
                                }
                                None => return Err("引号没有结束".into()),
                            }
                        }
                        Segment::Key(key)
                    }
                    _ => {
                        let start = i;
                        if chars.get(i) == Some(&'-') {
                            i += 1;
                        }
                        while chars.get(i).is_some_and(char::is_ascii_digit) {
                            i += 1;
                        }
                        let number: String = chars[start..i].iter().collect();
                        Segment::Index(
                            number
                                .parse()
                                .map_err(|_| "“[”后面应为数字下标或带引号的键".to_string())?,
                        )
                    }
                };
                while chars.get(i).is_some_and(|c| c.is_whitespace()) {
                    i += 1;
                }
                if chars.get(i) != Some(&']') {
                    return Err("缺少“]”".into());
                }
                i += 1;
                segments.push(segment);
            }
            c if c.is_whitespace() => i += 1,
            other => return Err(format!("无法识别“{other}”")),
        }
    }
    Ok(segments)
}

/// 按路径取值。`[]` 展开时，某个元素取不到值的位置为 `null`。
pub fn query<'a>(node: &Node<'a>, path: &[Segment]) -> Result<Node<'a>, String> {
    let Some((first, rest)) = path.split_first() else {
        return Ok(node.clone());
    };
    match first {
        Segment::Each => {
            let children: Vec<&Node<'a>> = match node {
                Node::Array(items) => items.iter().collect(),
                Node::Object(entries) => entries.iter().map(|(_, value)| value).collect(),
                Node::Scalar(_) => return Err("“[]”只能用于数组或对象".into()),
            };
            Ok(Node::Array(
                children
                    .into_iter()
                    .map(|child| query(child, rest).unwrap_or(Node::Scalar(Cow::Borrowed("null"))))
                    .collect(),
            ))
        }
        Segment::Key(key) => match node {
            Node::Object(entries) => {
                let value = entries
                    .iter()
                    .find(|(raw, _)| serde_json::from_str::<String>(raw).is_ok_and(|k| k == *key))
                    .map(|(_, value)| value)
                    .ok_or_else(|| format!("找不到键“{key}”"))?;
                query(value, rest)
            }
            Node::Array(items) if key == "length" => {
                query(&Node::Scalar(Cow::Owned(items.len().to_string())), rest)
            }
            Node::Array(_) => Err(format!("数组没有键“{key}”，请用 [下标] 或 [] 取元素")),
            Node::Scalar(text) => Err(format!("{text} 不是对象，无法读取“{key}”")),
        },
        Segment::Index(index) => match node {
            Node::Array(items) => {
                let len = items.len() as i64;
                let position = if *index < 0 { len + index } else { *index };
                let item = usize::try_from(position)
                    .ok()
                    .and_then(|position| items.get(position))
                    .ok_or_else(|| format!("下标 {index} 超出范围（数组长度为 {len}）"))?;
                query(item, rest)
            }
            _ => Err(format!("不是数组，无法使用下标 [{index}]")),
        },
    }
}

/// 对整段文本执行过滤，返回格式化后的结果。
pub fn filter(src: &str, expr: &str, indent: Indent) -> Result<String, String> {
    let root = parse(src).map_err(|err| err.to_string())?;
    let path = parse_path(expr)?;
    Ok(write_node(&query(&root, &path)?, indent))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"menuId": 701011,"menuName": "预算和投入","menuNameEn": "","parentId": 7010,"icon": "","routePath": "bugetInvest","sort": 2.0,"visible": true,"subList": [],"todoCount": 0}"#;

    #[test]
    fn formats_like_the_utools_screenshot() {
        let expected = r#"{
    "menuId": 701011,
    "menuName": "预算和投入",
    "menuNameEn": "",
    "parentId": 7010,
    "icon": "",
    "routePath": "bugetInvest",
    "sort": 2.0,
    "visible": true,
    "subList": [],
    "todoCount": 0
}"#;
        assert_eq!(format(SAMPLE, Indent::Four).unwrap(), expected);
    }

    #[test]
    fn keeps_number_literals_order_and_big_integers() {
        let src = r#"{"z":1.50,"a":12345678901234567890123,"e":-1E+5,"z":0}"#;
        assert_eq!(
            format(src, Indent::Two).unwrap(),
            "{\n  \"z\": 1.50,\n  \"a\": 12345678901234567890123,\n  \"e\": -1E+5,\n  \"z\": 0\n}"
        );
        assert_eq!(minify(&format(src, Indent::Tab).unwrap()).unwrap(), src);
    }

    #[test]
    fn formats_nested_and_empty_containers() {
        let src = "[1,[2,3],{},{ },[ ],{\"a\":{\"b\":[true,null]}}]";
        let expected = "[\n\t1,\n\t[\n\t\t2,\n\t\t3\n\t],\n\t{},\n\t{},\n\t[],\n\t{\n\t\t\"a\": {\n\t\t\t\"b\": [\n\t\t\t\ttrue,\n\t\t\t\tnull\n\t\t\t]\n\t\t}\n\t}\n]";
        assert_eq!(format(src, Indent::Tab).unwrap(), expected);
        assert_eq!(format("  42 ", Indent::Two).unwrap(), "42");
    }

    #[test]
    fn keeps_comments_when_formatting() {
        let src = "// 配置\n{\"a\":1, // 行尾注释\n/* 独占一行 */\n\"b\": /* 行内 */ [2]}";
        let expected = "// 配置\n{\n  \"a\": 1, // 行尾注释\n  /* 独占一行 */\n  \"b\": /* 行内 */ [\n    2\n  ]\n}";
        let formatted = format(src, Indent::Two).unwrap();
        assert_eq!(formatted, expected);
        // 再次格式化结果不变。
        assert_eq!(format(&formatted, Indent::Two).unwrap(), formatted);
        // 行注释后面紧跟逗号时，逗号必须换行，否则会被注释吞掉。
        let tricky = format("[1 // x\n, 2]", Indent::Two).unwrap();
        assert!(parse(&tricky).is_ok(), "{tricky}");
        assert_eq!(minify(&tricky).unwrap(), "[1,2]");
    }

    #[test]
    fn strips_comments() {
        let src = "{\"a\": \"http://x\" /* c */, // d\n\"b\": 2}";
        assert_eq!(strip_comments(src), "{\"a\": \"http://x\" , \n\"b\": 2}");
        assert_eq!(minify(src).unwrap(), "{\"a\":\"http://x\",\"b\":2}");
    }

    #[test]
    fn escapes_and_unescapes() {
        let escaped = escape("{\n  \"a\": \"引号\\\"\",\n  \"b\": [1, 2]\n}").unwrap();
        assert_eq!(escaped, r#"{\"a\":\"引号\\\"\",\"b\":[1,2]}"#);
        assert_eq!(
            unescape(&escaped).as_deref(),
            Some(r#"{"a":"引号\"","b":[1,2]}"#)
        );
        assert_eq!(unescape(r#""{\"a\":1}""#).as_deref(), Some(r#"{"a":1}"#));
        assert_eq!(unescape("{\"a\":1}"), None);
    }

    #[test]
    fn prepares_payloads() {
        assert_eq!(prepare("[1]", Indent::Two).text, "[\n  1\n]");
        let escaped = prepare(r#"{\"a\":1}"#, Indent::Two);
        assert_eq!(escaped.text, "{\n  \"a\": 1\n}");
        assert_eq!(escaped.note, Some("已去除转义"));
        let quoted = prepare(r#""{\"a\":[]}""#, Indent::Two);
        assert_eq!(quoted.text, "{\n  \"a\": []\n}");
        assert_eq!(quoted.note, Some("已去除一层转义"));
        // 普通字符串、非法 JSON 保持原样。
        assert_eq!(prepare("\"hello\"", Indent::Two).text, "\"hello\"");
        assert_eq!(prepare("{a:1}", Indent::Two).text, "{a:1}");
    }

    #[test]
    fn detects_json_like_selections() {
        assert!(looks_like_json(SAMPLE));
        assert!(looks_like_json("  [1, 2]\n"));
        assert!(looks_like_json("{oops}"), "与 uTools 相同，只看首尾括号");
        assert!(looks_like_json(r#""{\"a\":1}""#));
        assert!(!looks_like_json("hello"));
        assert!(!looks_like_json("{"));
        assert!(!looks_like_json("\"just a string\""));
    }

    fn error(src: &str) -> (usize, usize, String) {
        let err = parse(src).unwrap_err();
        (err.line, err.column, err.message)
    }

    #[test]
    fn reports_error_positions() {
        assert_eq!(error("{\n  \"a\": 1\n  \"b\": 2\n}").0, 3);
        assert_eq!(error("{\n  \"a\": 1\n  \"b\": 2\n}").1, 3);
        assert!(error("[1,]").2.contains("多余的逗号"));
        assert!(error("{'a': 1}").2.contains("双引号"));
        assert!(error("{\"a\": undefined}").2.contains("undefined"));
        assert!(error("[01]").2.contains("无效的数字"));
        assert!(error("[1.]").2.contains("无效的数字"));
        assert!(error("{\"a\": \"x").2.contains("没有结束"));
        assert!(error("\"\\x\"").2.contains("无效的转义"));
        assert!(error("\"tab\there\"").2.contains("制表符"));
        assert!(error("[1] 2").2.contains("多余的内容"));
        assert!(error("[1 /* x ]").2.contains("注释没有结束"));
        assert!(error("   ").2.contains("为空"));
        // 列号按字符计：中文算一列。
        assert_eq!(error("{\"中文\" 1}"), (1, 7, "键后面应为“:”".to_string()));
        assert!(parse(&"[".repeat(600))
            .unwrap_err()
            .message
            .contains("嵌套"));
        // 网页里的不换行空格按空白处理。
        assert!(parse("{\u{a0}\"a\":\u{a0}1}").is_ok());
    }

    #[test]
    fn tokens_cover_input_without_panicking() {
        for src in [
            "\"\\é\"",
            "\"abc\\",
            "\"多字节\\",
            "/*",
            "/* 未结束",
            "'x'",
            "\u{1F600}",
            "-",
            "\"\r\n",
        ] {
            let tokens = tokenize(src);
            let joined: String = tokens.iter().map(|t| t.text(src)).collect();
            assert_eq!(joined, src);
            let _ = parse(src);
            let _ = reformat(src, "  ");
        }
    }

    #[test]
    fn filters_by_path() {
        let src = r#"{"data": {"list": [{"name": "a", "n": 1.0}, {"name": "b"}], "my key": 2}}"#;
        assert_eq!(filter(src, ".data.list[0].n", Indent::Two).unwrap(), "1.0");
        assert_eq!(
            filter(src, "data.list[-1].name", Indent::Two).unwrap(),
            "\"b\""
        );
        assert_eq!(
            filter(src, "this.data[\"my key\"]", Indent::Two).unwrap(),
            "2"
        );
        assert_eq!(
            filter(src, ".data.list[].name", Indent::Two).unwrap(),
            "[\n  \"a\",\n  \"b\"\n]"
        );
        assert_eq!(
            filter(src, ".data.list[*].n", Indent::Two).unwrap(),
            "[\n  1.0,\n  null\n]"
        );
        assert_eq!(filter(src, ".data.list.length", Indent::Two).unwrap(), "2");
        assert_eq!(
            filter(src, "", Indent::Two).unwrap(),
            format(src, Indent::Two).unwrap()
        );
        assert!(filter(src, ".data.missing", Indent::Two)
            .unwrap_err()
            .contains("missing"));
        assert!(filter(src, ".data.list[5]", Indent::Two)
            .unwrap_err()
            .contains("超出范围"));
        assert!(filter(src, ".data.list.name", Indent::Two).is_err());
        assert!(filter(src, ".data[", Indent::Two).is_err());
        assert_eq!(
            parse_path("[0][1]").unwrap(),
            vec![Segment::Index(0), Segment::Index(1)]
        );
        assert_eq!(
            parse_path(".名称.*").unwrap(),
            vec![Segment::Key("名称".into()), Segment::Each]
        );
    }
}
