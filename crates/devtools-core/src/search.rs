use devtools_plugin_api::{CommandDescriptor, CommandSource};
use devtools_storage::ClipboardRecord;

/// UI 展示用的统一搜索结果。
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub source: SearchSource,
    pub score: f32,
}

/// 搜索结果来源，用于排序、分组和 UI 标签展示。
#[derive(Clone, Debug)]
pub enum SearchSource {
    BuiltInTool,
    Clipboard,
    Plugin,
    History,
    Setting,
}

/// 搜索引擎内部可索引的数据类型。
#[derive(Clone, Debug)]
pub enum SearchItem {
    Command(CommandDescriptor),
    Clipboard(ClipboardRecord),
}

/// 简单命令搜索引擎：阶段 1 先用关键词匹配，后续可替换为更强的索引。
#[derive(Clone, Debug)]
pub struct SearchEngine {
    commands: Vec<CommandDescriptor>,
}

impl SearchEngine {
    /// 用命令清单创建搜索引擎。
    pub fn new(commands: Vec<CommandDescriptor>) -> Self {
        Self { commands }
    }

    /// 搜索命令并按得分降序返回。
    pub fn search_commands(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let query = query.trim().to_ascii_lowercase();
        let mut results = self
            .commands
            .iter()
            .filter_map(|command| score_command(command, &query))
            .collect::<Vec<_>>();

        results.sort_by(|left, right| right.score.total_cmp(&left.score));
        results.truncate(limit);
        results
    }
}

/// 为单个命令打分：标题前缀命中最高，关键词包含次之。
fn score_command(command: &CommandDescriptor, query: &str) -> Option<SearchResult> {
    let haystack = format!(
        "{} {} {}",
        command.title,
        command.subtitle,
        command.keywords.join(" ")
    )
    .to_ascii_lowercase();

    let score = if query.is_empty() {
        0.45
    } else if command.title.to_ascii_lowercase().starts_with(query) {
        1.0
    } else if haystack.contains(query) {
        0.75
    } else {
        return None;
    };

    Some(SearchResult {
        id: command.id.clone(),
        title: command.title.clone(),
        subtitle: command.subtitle.clone(),
        source: source_from_command(&command.source),
        score,
    })
}

/// 将剪贴板记录转换为搜索结果，敏感内容只展示摘要。
pub fn clipboard_result(record: ClipboardRecord, score: f32) -> SearchResult {
    let display = if record.sensitive {
        mask_sensitive(&record.content)
    } else {
        summarize(&record.content)
    };

    SearchResult {
        id: format!("clipboard:{}", record.id),
        title: format!("Clipboard: {display}"),
        subtitle: match record.source_app {
            Some(source) if !source.trim().is_empty() => {
                format!("{} content copied from {source}", record.content_type)
            }
            _ => format!("{} content captured locally", record.content_type),
        },
        source: SearchSource::Clipboard,
        score,
    }
}

/// 将插件 API 的命令来源映射成 core 层来源，避免 UI 直接依赖插件枚举。
fn source_from_command(source: &CommandSource) -> SearchSource {
    match source {
        CommandSource::BuiltInTool => SearchSource::BuiltInTool,
        CommandSource::Plugin => SearchSource::Plugin,
        CommandSource::Clipboard => SearchSource::Clipboard,
        CommandSource::History => SearchSource::History,
        CommandSource::Setting => SearchSource::Setting,
    }
}

/// 把多行文本压成单行摘要，避免列表项被长文本撑坏。
fn summarize(content: &str) -> String {
    let single_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() > 72 {
        format!("{}...", single_line.chars().take(72).collect::<String>())
    } else {
        single_line
    }
}

/// 敏感内容只保留头部片段，降低误展示 token、密码等内容的风险。
fn mask_sensitive(content: &str) -> String {
    let summary = summarize(content);
    let head = summary.chars().take(12).collect::<String>();
    format!("{head}...")
}

#[cfg(test)]
mod tests {
    use devtools_plugin_api::{CommandDescriptor, CommandSource};

    use super::SearchEngine;

    /// 标题前缀命中的命令应该排在普通内容命中前面。
    #[test]
    fn ranks_title_prefix_matches_first() {
        let search = SearchEngine::new(vec![
            CommandDescriptor {
                id: "tool.json.format".into(),
                plugin_id: None,
                title: "JSON Format".into(),
                subtitle: "Format and validate JSON text".into(),
                i18n: vec![],
                keywords: vec!["pretty".into()],
                source: CommandSource::BuiltInTool,
            },
            CommandDescriptor {
                id: "tool.base64.encode".into(),
                plugin_id: None,
                title: "Base64 Encode".into(),
                subtitle: "Encode JSON payloads".into(),
                i18n: vec![],
                keywords: vec!["encode".into()],
                source: CommandSource::BuiltInTool,
            },
        ]);

        let results = search.search_commands("json", 10);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "tool.json.format");
    }
}
