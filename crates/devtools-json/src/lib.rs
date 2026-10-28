//! JSON 文本处理：校验、格式化、压缩、转义、去除注释、路径过滤。
//!
//! 与 uTools「JSON 编辑器」插件一样直接处理文本（插件用 `formatText` 加 Monaco 的
//! `editor.action.formatDocument` 格式化），而不是解析成值再序列化回去：
//! 数字字面量（`2.0`、超出 f64 精度的大整数）、键的顺序和重复的键都原样保留；
//! 允许 `//` 与 `/* */` 注释，格式化时保留，“去除注释”时删掉。
//!
//! 只依赖 `serde` / `serde_json`，不依赖界面与平台，可以单独测试。
//!
//! 分为几步：
//! - `lexer`：把任意文本切成词法单元（也用于编辑器的语法高亮）；
//! - `parser`：校验并解析成保留原文的语法树，报告错误位置；
//! - `format`：格式化（按词法单元重新排版）与按语法树输出；
//! - `path`：`.data.list[0]` 这样的路径过滤。
//!
//! 本文件是面向编辑器的各种操作。

mod format;
mod lexer;
mod parser;
mod path;
#[cfg(test)]
mod tests;

pub use format::{format, Indent};
pub use lexer::{tokenize, TokenKind};
pub use parser::{parse, JsonError, Node};
pub use path::filter;

use crate::format::reformat;

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
