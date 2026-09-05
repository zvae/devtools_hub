use super::*;

pub(super) fn decode_jwt(input: &str) -> CommandResult {
    let token = input.trim().strip_prefix("Bearer ").unwrap_or(input.trim());
    let parts = token.split('.').collect::<Vec<_>>();
    let content = if parts.len() != 3 {
        "Invalid JWT: expected three dot-separated parts".into()
    } else {
        let header = decode_base64url_json(parts[0], "header");
        let payload = decode_base64url_json(parts[1], "payload");
        format!(
            "Header:\n{header}\n\nPayload:\n{payload}\n\nSignature: {} bytes",
            parts[2].len()
        )
    };
    CommandResult {
        title: "JWT Decode".into(),
        content,
    }
}

pub(super) fn decode_base64url_json(value: &str, section: &str) -> String {
    general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| general_purpose::URL_SAFE.decode(value))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| format!("Invalid JWT {section}"))
}
