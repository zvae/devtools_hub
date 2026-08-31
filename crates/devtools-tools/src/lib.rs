use base64::{engine::general_purpose, Engine as _};
use devtools_plugin_api::{
    CommandAction, CommandDescriptor, CommandI18n, CommandResult, CommandSource,
};

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

pub fn interactive_tool_ids() -> Vec<String> {
    builtin_commands()
        .into_iter()
        .filter(|command| command.id.starts_with("tool.") && command.id != "tool.uuid.v4")
        .map(|command| command.id)
        .collect()
}

pub fn title_for(command_id: &str) -> String {
    localized_title_for(command_id, "en")
}

pub fn localized_title_for(command_id: &str, locale: &str) -> String {
    builtin_commands()
        .into_iter()
        .find(|command| command.id == command_id)
        .map(|command| localized_command_text(&command, locale).0)
        .unwrap_or_else(|| command_id.to_string())
}

pub fn localized_subtitle_for(command_id: &str, locale: &str) -> String {
    builtin_commands()
        .into_iter()
        .find(|command| command.id == command_id)
        .map(|command| localized_command_text(&command, locale).1)
        .unwrap_or_default()
}

pub fn localized_command_text(command: &CommandDescriptor, locale: &str) -> (String, String) {
    command
        .i18n
        .iter()
        .find(|text| text.locale == locale)
        .map(|text| (text.title.clone(), text.subtitle.clone()))
        .unwrap_or_else(|| (command.title.clone(), command.subtitle.clone()))
}

fn format_json(input: &str) -> CommandResult {
    let content = serde_json::from_str::<serde_json::Value>(input)
        .and_then(|value| serde_json::to_string_pretty(&value))
        .unwrap_or_else(|error| format!("Invalid JSON: {error}"));

    CommandResult {
        title: "JSON Format".into(),
        content,
    }
}

fn minify_json(input: &str) -> CommandResult {
    let content = serde_json::from_str::<serde_json::Value>(input)
        .and_then(|value| serde_json::to_string(&value))
        .unwrap_or_else(|error| format!("Invalid JSON: {error}"));

    CommandResult {
        title: "JSON Minify".into(),
        content,
    }
}

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

#[cfg(test)]
mod tests {
    use super::execute;

    #[test]
    fn formats_json() {
        let result = execute("tool.json.format", r#"{"hello":"world"}"#);
        assert!(result.content.contains("\"hello\": \"world\""));
    }

    #[test]
    fn decodes_base64() {
        let result = execute("tool.base64.decode", "aGVsbG8=");
        assert_eq!(result.content, "hello");
    }
}
