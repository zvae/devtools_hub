use super::*;

/// JSON 格式化：解析成功后输出 pretty JSON，失败时返回错误文本。
pub(super) fn format_json(input: &str) -> CommandResult {
    let content = serde_json::from_str::<serde_json::Value>(input)
        .and_then(|value| serde_json::to_string_pretty(&value))
        .unwrap_or_else(|error| format!("Invalid JSON: {error}"));

    CommandResult {
        title: "JSON Format".into(),
        content,
    }
}

/// JSON 压缩：解析成功后输出单行 JSON。
pub(super) fn minify_json(input: &str) -> CommandResult {
    let content = serde_json::from_str::<serde_json::Value>(input)
        .and_then(|value| serde_json::to_string(&value))
        .unwrap_or_else(|error| format!("Invalid JSON: {error}"));

    CommandResult {
        title: "JSON Minify".into(),
        content,
    }
}

/// JSON 校验：只报告是否合法，不改写输入内容。
pub(super) fn validate_json(input: &str) -> CommandResult {
    let content = match serde_json::from_str::<serde_json::Value>(input) {
        Ok(_) => "Valid JSON".to_string(),
        Err(error) => format!("Invalid JSON: {error}"),
    };

    CommandResult {
        title: "JSON Validate".into(),
        content,
    }
}
