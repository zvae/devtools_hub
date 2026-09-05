pub(super) fn percent_encode(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                vec![(byte as char).to_string()]
            } else {
                vec![format!("%{byte:02X}")]
            }
        })
        .collect()
}

pub(super) fn percent_decode(input: &str) -> Result<String, &'static str> {
    let mut bytes = Vec::with_capacity(input.len());
    let mut characters = input.as_bytes().iter().copied();
    while let Some(byte) = characters.next() {
        if byte == b'%' {
            let high = characters
                .next()
                .and_then(hex_value)
                .ok_or("incomplete escape")?;
            let low = characters
                .next()
                .and_then(hex_value)
                .ok_or("invalid escape")?;
            bytes.push(high << 4 | low);
        } else if byte == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|_| "decoded text is not UTF-8")
}

pub(super) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
