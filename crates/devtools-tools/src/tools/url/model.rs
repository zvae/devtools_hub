//! URL 百分号编码：按 UTF-8 字节编码为大写 `%XX`。
//! 三种规则分别对应浏览器的 encodeURIComponent、encodeURI 与 WHATWG 表单编码，便于和前端结果逐字对照。

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// 变体 0 组件、1 完整 URL、2 表单：决定哪些 ASCII 字符原样保留。
fn keep(byte: u8, variant: usize) -> bool {
    byte.is_ascii_alphanumeric()
        || match variant {
            1 => b"-_.!~*'();,/?:@&=+$#".contains(&byte),
            2 => b"-_.*".contains(&byte),
            _ => b"-_.!~*'()".contains(&byte),
        }
}

pub fn encode(text: &str, variant: usize) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if variant == 2 && byte == b' ' {
            out.push('+');
        } else if keep(byte, variant) {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 15)] as char);
        }
    }
    Ok(out)
}

/// 解码 `%XX`；只有表单规则把 `+` 当作空格，其余规则保留 `+`（与 decodeURIComponent 一致）。
pub fn decode(text: &str, variant: usize) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let pair = bytes
                    .get(i + 1..i + 3)
                    .and_then(|p| Some((hex(p[0])?, hex(p[1])?)));
                let Some((hi, lo)) = pair else {
                    return Err(format!(
                        "第 {} 个字符处的 % 后应为两位十六进制数",
                        text[..i].chars().count() + 1
                    ));
                };
                out.push((hi << 4) | lo);
                i += 3;
            }
            b'+' if variant == 2 => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|e| {
        format!(
            "解码结果不是有效的 UTF-8 文本（第 {} 个字节起）",
            e.utf8_error().valid_up_to() + 1
        )
    })
}

fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|d| d as u8)
}

/// 选中文本含合法的 `%XX` 且能解码时视为已编码；带 `+` 时按表单规则解码。
pub fn detect(text: &str) -> Option<usize> {
    let text = text.trim();
    let escaped = text
        .as_bytes()
        .windows(3)
        .any(|w| w[0] == b'%' && hex(w[1]).is_some() && hex(w[2]).is_some());
    let variant = if text.contains('+') { 2 } else { 0 };
    (escaped && decode(text, variant).is_ok()).then_some(variant)
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
