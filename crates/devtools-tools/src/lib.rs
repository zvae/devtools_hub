use base64::{engine::general_purpose, Engine as _};
use devtools_plugin_api::{
    CommandAction, CommandDescriptor, CommandI18n, CommandResult, CommandSource,
};

/// 内置工具清单。默认文本使用英文，中文通过 i18n 字段提供。
pub fn builtin_commands() -> Vec<CommandDescriptor> {
    vec![
        command(
            "tool.json.format",
            "JSON Format",
            "Format and validate JSON text",
            "JSON 格式化",
            "格式化并校验 JSON 文本",
            vec!["json", "format", "pretty", "格式化"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.json.minify",
            "JSON Minify",
            "Compress JSON into one line",
            "JSON 压缩",
            "将 JSON 压缩为单行文本",
            vec!["json", "minify", "compress", "压缩"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.json.validate",
            "JSON Validate",
            "Check whether JSON text is valid",
            "JSON 校验",
            "检查 JSON 文本是否合法",
            vec!["json", "validate", "lint", "校验"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.base64.encode",
            "Base64 Encode",
            "Encode text as Base64",
            "Base64 编码",
            "将文本编码为 Base64",
            vec!["base64", "encode", "编码"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.base64.decode",
            "Base64 Decode",
            "Decode Base64 into text",
            "Base64 解码",
            "将 Base64 解码为文本",
            vec!["base64", "decode", "解码"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.uuid.v4",
            "UUID v4",
            "Generate a random UUID",
            "UUID v4",
            "生成随机 UUID",
            vec!["uuid", "guid", "random"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.jwt.decode",
            "JWT Decode",
            "Decode JWT header and payload without sending it online",
            "JWT 解析",
            "离线解析 JWT Header 与 Payload",
            vec!["jwt", "token", "decode", "解析"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.url.encode",
            "URL Encode",
            "Percent-encode text for a URL component",
            "URL 编码",
            "将文本进行 URL 百分号编码",
            vec!["url", "encode", "percent", "编码"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.url.decode",
            "URL Decode",
            "Decode URL percent-encoded text",
            "URL 解码",
            "解码 URL 百分号编码文本",
            vec!["url", "decode", "percent", "解码"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.timestamp.convert",
            "Timestamp Convert",
            "Convert Unix seconds or ISO 8601 time",
            "时间戳转换",
            "转换 Unix 秒级时间戳或 ISO 8601 时间",
            vec!["timestamp", "time", "unix", "date", "时间"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.uuid.generate",
            "UUID Generator",
            "Generate UUID v4 values; enter a count from 1 to 100",
            "UUID 批量生成",
            "生成 UUID v4；输入 1 到 100 的数量",
            vec!["uuid", "guid", "batch", "generate", "批量"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.uuid.normalize",
            "UUID Normalize",
            "Normalize UUID case and hyphens",
            "UUID 规范化",
            "规范 UUID 的大小写和横线",
            vec!["uuid", "guid", "uppercase", "lowercase", "横线"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.sql.format",
            "SQL Format",
            "Format common SQL clauses for readable output",
            "SQL 格式化",
            "格式化常见 SQL 子句",
            vec!["sql", "format", "query", "select", "格式化"],
            CommandSource::BuiltInTool,
        ),
        command(
            "tool.clipboard.history",
            "Clipboard History",
            "Browse and search local clipboard items",
            "剪贴板历史",
            "浏览和搜索本地剪贴板记录",
            vec!["clipboard", "history", "paste", "剪贴板"],
            CommandSource::BuiltInTool,
        ),
        command(
            "setting.theme",
            "Settings",
            "Theme and local data preferences",
            "设置",
            "主题、语言、快捷键与本地数据配置",
            vec!["settings", "theme", "preferences", "设置"],
            CommandSource::Setting,
        ),
    ]
}

/// 创建内置命令描述，统一补齐中文本地化和关键词。
fn command(
    id: &str,
    title: &str,
    subtitle: &str,
    zh_title: &str,
    zh_subtitle: &str,
    keywords: Vec<&str>,
    source: CommandSource,
) -> CommandDescriptor {
    CommandDescriptor {
        id: id.into(),
        plugin_id: None,
        title: title.into(),
        subtitle: subtitle.into(),
        i18n: vec![CommandI18n {
            locale: "zh-CN".into(),
            title: zh_title.into(),
            subtitle: zh_subtitle.into(),
        }],
        keywords: keywords.into_iter().map(str::to_string).collect(),
        source,
    }
}

/// 将命令 ID 转成默认动作，供搜索结果激活时复用。
pub fn action_for(command_id: &str) -> CommandAction {
    match command_id {
        "tool.uuid.v4" => CommandAction::CopyText {
            text: uuid::Uuid::new_v4().to_string(),
        },
        id => CommandAction::OpenTool {
            tool_id: id.to_string(),
        },
    }
}

/// 执行内置工具。需要 UI 的工具只返回占位，实际由独立窗口承载。
pub fn execute(command_id: &str, input: &str) -> CommandResult {
    match command_id {
        "tool.json.format" => format_json(input),
        "tool.json.minify" => minify_json(input),
        "tool.json.validate" => validate_json(input),
        "tool.base64.encode" => CommandResult {
            title: "Base64 Encoded".into(),
            content: general_purpose::STANDARD.encode(input),
        },
        "tool.base64.decode" => {
            let content = general_purpose::STANDARD
                .decode(input)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_else(|| "Invalid Base64 input".into());

            CommandResult {
                title: "Base64 Decoded".into(),
                content,
            }
        }
        "tool.jwt.decode" => decode_jwt(input),
        "tool.url.encode" => CommandResult {
            title: "URL Encoded".into(),
            content: percent_encode(input),
        },
        "tool.url.decode" => CommandResult {
            title: "URL Decoded".into(),
            content: percent_decode(input)
                .unwrap_or_else(|error| format!("Invalid URL encoding: {error}")),
        },
        "tool.timestamp.convert" => convert_timestamp(input),
        "tool.uuid.generate" => generate_uuids(input),
        "tool.uuid.normalize" => normalize_uuid(input),
        "tool.sql.format" => format_sql(input),
        "tool.uuid.v4" => CommandResult {
            title: "UUID v4".into(),
            content: uuid::Uuid::new_v4().to_string(),
        },
        "tool.clipboard.history" => CommandResult {
            title: "Clipboard History".into(),
            content: String::new(),
        },
        _ => CommandResult {
            title: "Unknown command".into(),
            content: command_id.into(),
        },
    }
}

/// 返回需要打开交互窗口的工具 ID，排除 UUID 这种立即复制型工具。
pub fn interactive_tool_ids() -> Vec<String> {
    builtin_commands()
        .into_iter()
        .filter(|command| command.id.starts_with("tool.") && command.id != "tool.uuid.v4")
        .map(|command| command.id)
        .collect()
}

/// 英文标题兜底接口，保留给暂未传入语言的调用方。
pub fn title_for(command_id: &str) -> String {
    localized_title_for(command_id, "en")
}

/// 按语言查找命令标题。
pub fn localized_title_for(command_id: &str, locale: &str) -> String {
    builtin_commands()
        .into_iter()
        .find(|command| command.id == command_id)
        .map(|command| localized_command_text(&command, locale).0)
        .unwrap_or_else(|| command_id.to_string())
}

/// 按语言查找命令副标题。
pub fn localized_subtitle_for(command_id: &str, locale: &str) -> String {
    builtin_commands()
        .into_iter()
        .find(|command| command.id == command_id)
        .map(|command| localized_command_text(&command, locale).1)
        .unwrap_or_default()
}

/// 从命令描述中选择本地化文本，找不到指定语言时回退到默认文本。
pub fn localized_command_text(command: &CommandDescriptor, locale: &str) -> (String, String) {
    command
        .i18n
        .iter()
        .find(|text| text.locale == locale)
        .map(|text| (text.title.clone(), text.subtitle.clone()))
        .unwrap_or_else(|| (command.title.clone(), command.subtitle.clone()))
}

/// JSON 格式化：解析成功后输出 pretty JSON，失败时返回错误文本。
fn format_json(input: &str) -> CommandResult {
    let content = serde_json::from_str::<serde_json::Value>(input)
        .and_then(|value| serde_json::to_string_pretty(&value))
        .unwrap_or_else(|error| format!("Invalid JSON: {error}"));

    CommandResult {
        title: "JSON Format".into(),
        content,
    }
}

/// JSON 压缩：解析成功后输出单行 JSON。
fn minify_json(input: &str) -> CommandResult {
    let content = serde_json::from_str::<serde_json::Value>(input)
        .and_then(|value| serde_json::to_string(&value))
        .unwrap_or_else(|error| format!("Invalid JSON: {error}"));

    CommandResult {
        title: "JSON Minify".into(),
        content,
    }
}

/// JSON 校验：只报告是否合法，不改写输入内容。
fn validate_json(input: &str) -> CommandResult {
    let content = match serde_json::from_str::<serde_json::Value>(input) {
        Ok(_) => "Valid JSON".to_string(),
        Err(error) => format!("Invalid JSON: {error}"),
    };

    CommandResult {
        title: "JSON Validate".into(),
        content,
    }
}

fn decode_jwt(input: &str) -> CommandResult {
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

fn decode_base64url_json(value: &str, section: &str) -> String {
    general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| general_purpose::URL_SAFE.decode(value))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| format!("Invalid JWT {section}"))
}

fn percent_encode(input: &str) -> String {
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

fn percent_decode(input: &str) -> Result<String, &'static str> {
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

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn convert_timestamp(input: &str) -> CommandResult {
    let value = input.trim();
    let content = if value.is_empty() {
        format!(
            "Unix: {}\n+08:00: {}",
            timestamp_now(),
            timestamp_now_datetime()
        )
    } else if let Ok(seconds) = value.parse::<i64>() {
        timestamp_to_datetime(seconds)
            .map(|datetime| format!("Unix: {seconds}\n+08:00: {datetime}"))
            .unwrap_or_else(|error| format!("Invalid Unix timestamp: {error}"))
    } else {
        timestamp_from_datetime(value)
            .map(|timestamp| format!("Unix: {timestamp}\n+08:00 input"))
            .unwrap_or_else(|error| format!("Invalid +08:00 datetime: {error}"))
    };
    CommandResult {
        title: "Timestamp Convert".into(),
        content,
    }
}

/// 返回当前秒级或毫秒级时间戳。时间戳本身始终基于 UTC，因此无需时区换算。
pub fn timestamp_now() -> String {
    timestamp_now_with_unit(true)
}

pub fn timestamp_now_with_unit(milliseconds: bool) -> String {
    let now = time::OffsetDateTime::now_utc();
    if milliseconds {
        (now.unix_timestamp_nanos() / 1_000_000).to_string()
    } else {
        now.unix_timestamp().to_string()
    }
}

/// 将当前时间格式化为固定 +08:00 时区的本地日期时间。
pub fn timestamp_now_datetime() -> String {
    format_east8(time::OffsetDateTime::now_utc())
}

/// 将 Unix 秒级时间戳转换为 +08:00 日期时间。
pub fn timestamp_to_datetime(seconds: i64) -> Result<String, String> {
    time::OffsetDateTime::from_unix_timestamp(seconds)
        .map(format_east8)
        .map_err(|error| error.to_string())
}

/// 自动识别秒级和毫秒级时间戳。13 位左右的现代时间戳按毫秒处理。
pub fn timestamp_to_datetime_auto(value: i64) -> Result<String, String> {
    let seconds = if value.unsigned_abs() >= 100_000_000_000 {
        value / 1_000
    } else {
        value
    };
    timestamp_to_datetime(seconds)
}

/// 将固定 +08:00 的 `YYYY-MM-DD HH:MM:SS` 日期时间转换为 Unix 秒级时间戳。
pub fn timestamp_from_datetime(value: &str) -> Result<String, String> {
    use time::{format_description::FormatItem, macros::format_description, PrimitiveDateTime};

    static FORMAT: &[FormatItem<'static>] =
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    PrimitiveDateTime::parse(value.trim(), FORMAT)
        .map(|value| {
            value
                .assume_offset(east8_offset())
                .unix_timestamp()
                .to_string()
        })
        .map_err(|error| error.to_string())
}

pub fn timestamp_from_datetime_with_unit(
    value: &str,
    milliseconds: bool,
) -> Result<String, String> {
    timestamp_from_datetime(value).and_then(|seconds| convert_timestamp_unit(seconds, milliseconds))
}

/// 将分段的 +08:00 日期时间转换为 Unix 秒级时间戳。
pub fn timestamp_from_parts(
    year: &str,
    month: &str,
    day: &str,
    hour: &str,
    minute: &str,
    second: &str,
) -> Result<String, String> {
    use time::{Date, Month, PrimitiveDateTime, Time};

    let year = year
        .trim()
        .parse::<i32>()
        .map_err(|_| "year must be a number")?;
    let month = month
        .trim()
        .parse::<u8>()
        .map_err(|_| "month must be a number")?;
    let day = day
        .trim()
        .parse::<u8>()
        .map_err(|_| "day must be a number")?;
    let hour = hour
        .trim()
        .parse::<u8>()
        .map_err(|_| "hour must be a number")?;
    let minute = minute
        .trim()
        .parse::<u8>()
        .map_err(|_| "minute must be a number")?;
    let second = second
        .trim()
        .parse::<u8>()
        .map_err(|_| "second must be a number")?;
    let month = Month::try_from(month).map_err(|error| error.to_string())?;
    let date = Date::from_calendar_date(year, month, day).map_err(|error| error.to_string())?;
    let time = Time::from_hms(hour, minute, second).map_err(|error| error.to_string())?;
    Ok(PrimitiveDateTime::new(date, time)
        .assume_offset(east8_offset())
        .unix_timestamp()
        .to_string())
}

pub fn timestamp_from_parts_with_unit(
    year: &str,
    month: &str,
    day: &str,
    hour: &str,
    minute: &str,
    second: &str,
    milliseconds: bool,
) -> Result<String, String> {
    timestamp_from_parts(year, month, day, hour, minute, second)
        .and_then(|seconds| convert_timestamp_unit(seconds, milliseconds))
}

fn convert_timestamp_unit(seconds: String, milliseconds: bool) -> Result<String, String> {
    if !milliseconds {
        return Ok(seconds);
    }
    seconds
        .parse::<i64>()
        .map_err(|error| error.to_string())
        .and_then(|value| {
            value
                .checked_mul(1_000)
                .map(|value| value.to_string())
                .ok_or_else(|| "timestamp is out of range".to_string())
        })
}

fn east8_offset() -> time::UtcOffset {
    time::UtcOffset::from_hms(8, 0, 0).expect("+08:00 must be a valid UTC offset")
}

fn format_east8(value: time::OffsetDateTime) -> String {
    use time::{format_description::FormatItem, macros::format_description};

    static FORMAT: &[FormatItem<'static>] =
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    value
        .to_offset(east8_offset())
        .format(FORMAT)
        .unwrap_or_default()
}

fn generate_uuids(input: &str) -> CommandResult {
    let count = input.trim().parse::<usize>().unwrap_or(1).clamp(1, 100);
    let content = (0..count)
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    CommandResult {
        title: "UUID Generator".into(),
        content,
    }
}

fn normalize_uuid(input: &str) -> CommandResult {
    let compact = input.trim().replace('-', "");
    let content = match uuid::Uuid::parse_str(&compact) {
        Ok(value) => format!(
            "Lowercase: {}\nUppercase: {}\nCompact: {}",
            value.hyphenated(),
            value.hyphenated().to_string().to_ascii_uppercase(),
            value.simple()
        ),
        Err(error) => format!("Invalid UUID: {error}"),
    };
    CommandResult {
        title: "UUID Normalize".into(),
        content,
    }
}

fn format_sql(input: &str) -> CommandResult {
    let normalized = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut formatted = normalized;
    for keyword in [
        "SELECT",
        "FROM",
        "WHERE",
        "GROUP BY",
        "ORDER BY",
        "HAVING",
        "LIMIT",
        "LEFT JOIN",
        "RIGHT JOIN",
        "INNER JOIN",
        "JOIN",
        "UNION",
    ] {
        formatted = replace_case_insensitive(&formatted, keyword, &format!("\n{keyword}"));
    }
    CommandResult {
        title: "SQL Format".into(),
        content: formatted.trim_start().replace(", ", ",\n  "),
    }
}

fn replace_case_insensitive(input: &str, needle: &str, replacement: &str) -> String {
    let mut result = String::new();
    let mut rest = input;
    let lower_needle = needle.to_ascii_lowercase();
    while let Some(index) = rest.to_ascii_lowercase().find(&lower_needle) {
        result.push_str(&rest[..index]);
        result.push_str(replacement);
        rest = &rest[index + needle.len()..];
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::{
        execute, timestamp_from_datetime, timestamp_from_datetime_with_unit, timestamp_from_parts,
        timestamp_to_datetime, timestamp_to_datetime_auto,
    };

    /// JSON 格式化应输出带缩进和空格的 pretty JSON。
    #[test]
    fn formats_json() {
        let result = execute("tool.json.format", r#"{"hello":"world"}"#);
        assert!(result.content.contains("\"hello\": \"world\""));
    }

    /// Base64 解码应返回原始 UTF-8 文本。
    #[test]
    fn decodes_base64() {
        let result = execute("tool.base64.decode", "aGVsbG8=");
        assert_eq!(result.content, "hello");
    }

    #[test]
    fn decodes_url_text() {
        let result = execute("tool.url.decode", "hello%20world%21");
        assert_eq!(result.content, "hello world!");
    }

    #[test]
    fn decodes_jwt_payload_without_verifying_it_online() {
        let token = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJkZXZ0b29scyJ9.signature";
        let result = execute("tool.jwt.decode", token);
        assert!(result.content.contains("devtools"));
    }

    #[test]
    fn generates_requested_number_of_uuids() {
        let result = execute("tool.uuid.generate", "3");
        assert_eq!(result.content.lines().count(), 3);
    }

    #[test]
    fn converts_timestamps_in_the_fixed_east8_timezone() {
        assert_eq!(timestamp_to_datetime(0).unwrap(), "1970-01-01 08:00:00");
        assert_eq!(timestamp_from_datetime("1970-01-01 08:00:00").unwrap(), "0");
        assert_eq!(
            timestamp_from_parts("1970", "1", "1", "8", "0", "0").unwrap(),
            "0"
        );
    }

    #[test]
    fn recognizes_millisecond_timestamps_and_can_emit_milliseconds() {
        assert_eq!(
            timestamp_to_datetime_auto(1_700_000_000_000).unwrap(),
            timestamp_to_datetime(1_700_000_000).unwrap()
        );
        assert_eq!(
            timestamp_from_datetime_with_unit("1970-01-01 08:00:01", true).unwrap(),
            "1000"
        );
    }
}
