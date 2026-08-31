use devtools_plugin_api::{CommandDescriptor, CommandSource};
use devtools_storage::ClipboardRecord;

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub source: SearchSource,
    pub score: f32,
}

#[derive(Clone, Debug)]
pub enum SearchSource {
    BuiltInTool,
    Clipboard,
    Plugin,
    History,
    Setting,
}

#[derive(Clone, Debug)]
pub enum SearchItem {
    Command(CommandDescriptor),
    Clipboard(ClipboardRecord),
}

#[derive(Clone, Debug)]
pub struct SearchEngine {
    commands: Vec<CommandDescriptor>,
}

impl SearchEngine {
    pub fn new(commands: Vec<CommandDescriptor>) -> Self {
        Self { commands }
    }

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

fn source_from_command(source: &CommandSource) -> SearchSource {
    match source {
        CommandSource::BuiltInTool => SearchSource::BuiltInTool,
        CommandSource::Plugin => SearchSource::Plugin,
        CommandSource::Clipboard => SearchSource::Clipboard,
        CommandSource::History => SearchSource::History,
        CommandSource::Setting => SearchSource::Setting,
    }
}

fn summarize(content: &str) -> String {
    let single_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() > 72 {
        format!("{}...", single_line.chars().take(72).collect::<String>())
    } else {
        single_line
    }
}

fn mask_sensitive(content: &str) -> String {
    let summary = summarize(content);
    let head = summary.chars().take(12).collect::<String>();
    format!("{head}...")
}

#[cfg(test)]
mod tests {
    use devtools_plugin_api::{CommandDescriptor, CommandSource};

    use super::SearchEngine;

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
