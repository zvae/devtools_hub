//! 文本与 UTF-8 字节序列互转，支持 `\xHH` 转义、十六进制和十进制三种写法。
//! 解码按所选格式解析字节，再严格校验 UTF-8，出错时指出第几个字节开始无效。

/// 变体 0 `\xHH` 转义、1 空格分隔的十六进制、2 逗号分隔的十进制。
pub fn encode(text: &str, variant: usize) -> Result<String, String> {
    let bytes = text.as_bytes();
    Ok(match variant {
        1 => bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" "),
        2 => bytes
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        _ => bytes.iter().map(|b| format!("\\x{b:02X}")).collect(),
    })
}

pub fn decode(text: &str, variant: usize) -> Result<String, String> {
    let bytes = match variant {
        1 => parse_hex(text)?,
        2 => parse_decimal(text)?,
        _ => parse_escaped(text)?,
    };
    String::from_utf8(bytes).map_err(|e| {
        format!(
            "不是有效的 UTF-8 字节序列（第 {} 个字节起）",
            e.utf8_error().valid_up_to() + 1
        )
    })
}

/// `\xHH` 转义为字节，其他字符按自身的 UTF-8 字节保留，因此 `abc\xE4\xB8\xAD` 也能还原。
fn parse_escaped(text: &str) -> Result<Vec<u8>, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && matches!(bytes.get(i + 1), Some(b'x' | b'X')) {
            let pair = bytes
                .get(i + 2..i + 4)
                .and_then(|p| Some((hex(p[0])?, hex(p[1])?)));
            let Some((hi, lo)) = pair else {
                return Err(format!(
                    "第 {} 个字符处的 \\x 后应为两位十六进制数",
                    text[..i].chars().count() + 1
                ));
            };
            out.push((hi << 4) | lo);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Ok(out)
}

fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|d| d as u8)
}

/// 按空白与逗号切分，去掉首尾方括号，便于直接粘贴数组字面量。
fn tokens<'a>(text: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    text.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|token| !token.is_empty())
}

fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for token in tokens(text) {
        let digits = token
            .strip_prefix("0x")
            .or_else(|| token.strip_prefix("0X"))
            .unwrap_or(token);
        if digits.is_empty()
            || !digits.len().is_multiple_of(2)
            || !digits.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(format!("“{token}”不是成对的十六进制数字"));
        }
        for pair in digits.as_bytes().chunks(2) {
            if let (Some(hi), Some(lo)) = (hex(pair[0]), hex(pair[1])) {
                out.push((hi << 4) | lo);
            }
        }
    }
    Ok(out)
}

fn parse_decimal(text: &str) -> Result<Vec<u8>, String> {
    tokens(text)
        .map(|token| {
            token
                .parse::<u8>()
                .map_err(|_| format!("“{token}”不是 0–255 的整数"))
        })
        .collect()
}

/// 选中内容像 UTF-8 字节序列时返回对应格式。要求解码结果含非 ASCII 字符且没有控制字符，
/// 避免把时间戳、普通数字或十六进制串误判为字节序列。
pub fn detect(text: &str) -> Option<usize> {
    let text = text.trim();
    let candidates: &[usize] = if text.contains("\\x") || text.contains("\\X") {
        &[0]
    } else {
        &[1, 2]
    };
    candidates.iter().copied().find(|&variant| {
        decode(text, variant).is_ok_and(|decoded| {
            !decoded.is_ascii()
                && !decoded
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        })
    })
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
