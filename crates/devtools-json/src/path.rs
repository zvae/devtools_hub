//! 路径过滤：`.data.list[0].name`、`[]` 展开数组等，语法参照 uTools「JSON 编辑器」插件。

use std::borrow::Cow;

use super::format::{write_node, Indent};
use super::parser::{parse, Node};

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
