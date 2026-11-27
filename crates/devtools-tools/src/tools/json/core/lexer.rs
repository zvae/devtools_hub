//! 词法分析。

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
/// 一个词法单元：种类与在原文中的范围。
pub struct Token {
    pub kind: TokenKind,
    /// 字节偏移，总是落在字符边界上。
    pub start: usize,
    pub end: usize,
}

impl Token {
    /// 词法单元的原文。
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
