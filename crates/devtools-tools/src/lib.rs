//! Built-in tool catalog, dispatch, and shared public APIs.

use base64::{engine::general_purpose, Engine as _};
use devtools_plugin_api::{
    CommandAction, CommandDescriptor, CommandI18n, CommandResult, CommandSource,
};

#[path = "tools/catalog.rs"]
mod catalog;
#[path = "tools/json.rs"]
mod json;
#[path = "tools/jwt.rs"]
mod jwt;
#[path = "tools/sql.rs"]
mod sql;
#[path = "tools/timestamp.rs"]
mod timestamp;
#[path = "tools/url.rs"]
mod url;
#[path = "tools/uuid.rs"]
mod uuid_tool;

pub use catalog::builtin_commands;
pub use timestamp::{
    timestamp_from_datetime, timestamp_from_datetime_with_unit, timestamp_from_parts,
    timestamp_from_parts_with_unit, timestamp_now, timestamp_now_datetime, timestamp_now_with_unit,
    timestamp_to_datetime, timestamp_to_datetime_auto,
};

use json::*;
use jwt::*;
use sql::*;
use timestamp::*;
use url::*;
use uuid_tool::*;

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
