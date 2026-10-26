//! 格式化输出。

use super::lexer::{tokenize, TokenKind};
use super::parser::{parse, JsonError, Node};

/// 缩进方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Indent {
    Two,
    #[default]
    Four,
    Tab,
}

impl Indent {
    /// 设置里可选的全部缩进方式。
    pub const ALL: [Indent; 3] = [Indent::Two, Indent::Four, Indent::Tab];

    /// 一级缩进的文本。
    pub fn unit(self) -> &'static str {
        match self {
            Indent::Two => "  ",
            Indent::Four => "    ",
            Indent::Tab => "\t",
        }
    }

    /// 界面上显示的名字。
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
pub(super) fn reformat(src: &str, indent: &str) -> String {
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

/// 把节点按指定缩进输出。
pub fn write_node(node: &Node<'_>, indent: Indent) -> String {
    let mut out = String::new();
    write_into(node, indent.unit(), 0, &mut out);
    out
}

/// 递归输出节点，`depth` 是当前所在的缩进层数。
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
