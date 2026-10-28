//! 校验与解析：严格按 JSON 语法（另外允许注释），出错时给出行列号与中文说明。

use std::borrow::Cow;
use std::fmt;

use super::lexer::{tokenize, Token, TokenKind};

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
    /// 在原文的 `offset` 处报错，换算出行列号。
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

/// 递归下降解析器，只处理去掉空白和注释后的词法单元。
struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    /// 下一个要读的词法单元。
    pos: usize,
    /// 当前的嵌套层数。
    depth: usize,
}

impl<'a> Parser<'a> {
    /// 读取下一个词法单元（读完后返回 `None`）。
    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).copied();
        self.pos += 1;
        token
    }

    /// 下一个词法单元是否是指定的标点（不读取）。
    fn peek_is(&self, punct: u8) -> bool {
        self.tokens
            .get(self.pos)
            .is_some_and(|t| t.kind == TokenKind::Punct(punct))
    }

    fn error(&self, offset: usize, message: impl Into<String>) -> JsonError {
        JsonError::at(self.src, offset, message)
    }

    /// 在文本末尾报错（内容提前结束）。
    fn eof(&self, message: &str) -> JsonError {
        self.error(self.src.len(), message)
    }

    /// 解析一个值：对象、数组或标量。
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

    /// 进入一层对象或数组，超过 [`MAX_DEPTH`] 时报错。
    fn enter(&mut self, open: Token) -> Result<(), JsonError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error(open.start, format!("嵌套超过 {MAX_DEPTH} 层")));
        }
        Ok(())
    }

    /// 解析对象，`open` 是已经读过的 `{`。
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

    /// 解析数组，`open` 是已经读过的 `[`。
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

    /// 检查字符串的转义与控制字符（词法分析只负责切分，不检查内容）。
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
