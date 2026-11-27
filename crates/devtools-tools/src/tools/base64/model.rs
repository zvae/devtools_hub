//! Base64 编解码（RFC 4648）：不引入新依赖，文本一律按 UTF-8 处理。
//! 解码同时接受标准与 URL 安全两种字母表，忽略空白与换行（兼容 MIME 折行），填充可省略。

const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// 变体 0 为标准字母表并补 `=`；变体 1 为 URL 安全字母表且不补 `=`（JWT 等场景的惯例）。
pub fn encode(text: &str, variant: usize) -> Result<String, String> {
    Ok(encode_bytes(text.as_bytes(), variant == 1))
}

pub fn encode_bytes(bytes: &[u8], url_safe: bool) -> String {
    let (alphabet, pad) = if url_safe {
        (URL_SAFE, false)
    } else {
        (STANDARD, true)
    };
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        // n 个字节产生 n + 1 个有效字符，其余位置按需补 `=`。
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(alphabet[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else if pad {
                out.push('=');
            }
        }
    }
    out
}

/// 解码为文本；结果不是 UTF-8 时提示可能是二进制数据，而不是显示乱码。
pub fn decode(text: &str, _variant: usize) -> Result<String, String> {
    let bytes = decode_bytes(text)?;
    let len = bytes.len();
    String::from_utf8(bytes)
        .map_err(|_| format!("解码得到 {len} 字节的二进制数据，不是有效的 UTF-8 文本"))
}

pub fn decode_bytes(text: &str) -> Result<Vec<u8>, String> {
    let mut values = Vec::with_capacity(text.len());
    let mut padding = 0usize;
    for (index, ch) in text.chars().enumerate() {
        if ch.is_ascii_whitespace() {
            continue;
        }
        if ch == '=' {
            padding += 1;
            continue;
        }
        if padding > 0 {
            return Err("= 填充只能出现在末尾".into());
        }
        let value = match ch {
            'A'..='Z' => ch as u8 - b'A',
            'a'..='z' => ch as u8 - b'a' + 26,
            '0'..='9' => ch as u8 - b'0' + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            _ => return Err(format!("第 {} 个字符“{ch}”不是 Base64 字符", index + 1)),
        };
        values.push(value);
    }
    if padding > 2 || (padding > 0 && !(values.len() + padding).is_multiple_of(4)) {
        return Err("= 填充数量不正确".into());
    }
    if values.len() % 4 == 1 {
        return Err("Base64 长度不正确，可能缺少或多出字符".into());
    }
    let mut out = Vec::with_capacity(values.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for value in values {
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// 选中文本是否像 Base64：不含空白、长度合法，且解码后是可读的 UTF-8 文本。
/// 要求可读是为了避免普通单词（如 `test`）被误判；返回解码时应选中的字母表。
pub fn detect(text: &str) -> Option<usize> {
    let text = text.trim();
    let url_safe = text.contains(['-', '_']);
    if text.len() < 4
        || text.chars().any(|c| c.is_whitespace())
        || !(text.len().is_multiple_of(4) || (url_safe && text.len() % 4 != 1))
    {
        return None;
    }
    let decoded = decode(text, 0).ok()?;
    let readable = !decoded.is_empty()
        && !decoded
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'));
    readable.then_some(usize::from(url_safe))
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
