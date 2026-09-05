use super::*;

/// 应用主题到主窗口状态。
pub(super) fn apply_theme(app: &SearchWindow, theme: &str) {
    app.set_settings_theme(theme.into());
    app.set_dark_mode(theme != "light");
}

/// 根据当前语言选择状态提示文本。
pub(super) fn localized_status(app: &SearchWindow, zh: &str, en: &str) -> SharedString {
    if app.get_language() == "en" {
        en.into()
    } else {
        zh.into()
    }
}

/// 从行索引查找真实结果 ID。
pub(super) fn get_id(ids: &Arc<Mutex<Vec<String>>>, index: i32) -> Option<String> {
    ids.lock()
        .ok()
        .and_then(|ids| ids.get(index as usize).cloned())
}

/// 更新行索引到结果 ID 的映射。
pub(super) fn set_ids(ids: &Arc<Mutex<Vec<String>>>, values: impl Iterator<Item = String>) {
    if let Ok(mut ids) = ids.lock() {
        *ids = values.collect();
    }
}

/// 本次启动内用过的工具优先；最近一次使用优先，再按累计使用次数排序。
pub(super) fn sort_tool_commands(
    commands: &mut [CommandDescriptor],
    usage: &Arc<Mutex<HashMap<String, ToolUsage>>>,
) {
    let Ok(usage) = usage.lock() else {
        return;
    };
    commands.sort_by(
        |left, right| match (usage.get(&left.id), usage.get(&right.id)) {
            (Some(left_usage), Some(right_usage)) => right_usage
                .last_used
                .cmp(&left_usage.last_used)
                .then_with(|| right_usage.total.cmp(&left_usage.total))
                .then_with(|| left.title.cmp(&right.title)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.title.cmp(&right.title),
        },
    );
}

pub(super) fn record_tool_usage(usage: &Arc<Mutex<HashMap<String, ToolUsage>>>, tool_id: &str) {
    if let Ok(mut usage) = usage.lock() {
        let next_sequence = usage
            .values()
            .map(|entry| entry.last_used)
            .max()
            .unwrap_or(0)
            + 1;
        let entry = usage.entry(tool_id.to_string()).or_default();
        entry.last_used = next_sequence;
        entry.total += 1;
    }
}

pub(super) fn refresh_tool_list(
    app: &SearchWindow,
    tool_ids: &Arc<Mutex<Vec<String>>>,
    commands: &Arc<Vec<CommandDescriptor>>,
    usage: &Arc<Mutex<HashMap<String, ToolUsage>>>,
) {
    let mut commands = commands
        .iter()
        .filter(|command| command.id != "setting.theme")
        .cloned()
        .collect::<Vec<_>>();
    sort_tool_commands(&mut commands, usage);
    set_ids(tool_ids, commands.iter().map(|command| command.id.clone()));
    let language = app.get_language().to_string();
    app.set_tools(command_model(commands, &language));
}

pub(super) fn tool_history_model(records: &[ToolHistoryRecord]) -> ModelRc<ToolHistoryView> {
    let rows = records
        .iter()
        .map(|record| ToolHistoryView {
            summary: summarize(&record.input).into(),
            created_at: format_clipboard_time(record.created_at).into(),
        })
        .collect::<Vec<_>>();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// 将命令描述符转换成 Slint 列表模型。
pub(super) fn command_model(
    commands: Vec<CommandDescriptor>,
    language: &str,
) -> ModelRc<SearchResultView> {
    let rows = commands
        .into_iter()
        .map(|command| {
            let (title, subtitle) = devtools_tools::localized_command_text(&command, language);
            SearchResultView {
                id: command.id.into(),
                title: title.into(),
                subtitle: subtitle.into(),
                source: source_label_from_command(&command.source, language),
                meta: "".into(),
                shortcut: "".into(),
            }
        })
        .collect::<Vec<_>>();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// 将剪贴板记录转换成 Slint 列表模型，并按语言显示来源标签。
pub(super) fn clipboard_model(
    records: Vec<ClipboardRecord>,
    language: &str,
) -> ModelRc<SearchResultView> {
    let rows = records
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            let source_app = record.source_app.unwrap_or_else(|| {
                if language == "en" {
                    "Local clipboard".into()
                } else {
                    "本地剪贴板".into()
                }
            });
            let title = if record.sensitive {
                format!("{}...", record.content.chars().take(12).collect::<String>())
            } else {
                summarize(&record.content)
            };
            let source = source_app.chars().take(2).collect::<String>();
            SearchResultView {
                id: format!("clipboard:{}", record.id).into(),
                title: title.into(),
                subtitle: source_app.into(),
                source: source.into(),
                meta: format_clipboard_time(record.created_at).into(),
                shortcut: clipboard_shortcut(index).into(),
            }
        })
        .collect::<Vec<_>>();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

pub(super) fn clipboard_count(count: usize, language: &str) -> String {
    if language == "en" {
        format!("{count} items")
    } else {
        format!("共 {count} 项")
    }
}

pub(super) fn format_clipboard_time(timestamp: i64) -> String {
    let Ok(utc_time) = OffsetDateTime::from_unix_timestamp(timestamp) else {
        return String::new();
    };
    let east8 = UtcOffset::from_hms(8, 0, 0).expect("+08:00 must be valid");
    utc_time
        .to_offset(east8)
        .format(&format_description!("[hour]:[minute]"))
        .unwrap_or_default()
}

pub(super) fn clipboard_shortcut(index: usize) -> String {
    if index >= 9 {
        return String::new();
    }
    let modifier = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl"
    };
    format!("{modifier} {}", index + 1)
}

/// 将核心搜索结果转换成 Slint 列表模型。
pub(super) fn results_to_model(
    results: Vec<SearchResult>,
    language: &str,
) -> ModelRc<SearchResultView> {
    let rows = results
        .into_iter()
        .map(|result| {
            let title = localized_title(&result.id, language, &result.title);
            let subtitle = localized_subtitle(&result.id, language, &result.subtitle);
            SearchResultView {
                id: result.id.into(),
                title: title.into(),
                subtitle: subtitle.into(),
                source: source_label(&result.source, language),
                meta: "".into(),
                shortcut: "".into(),
            }
        })
        .collect::<Vec<_>>();

    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// 将命令来源本地化成短标签，显示在结果行左侧徽标中。
pub(super) fn source_label_from_command(source: &CommandSource, language: &str) -> SharedString {
    match source {
        CommandSource::BuiltInTool => if language == "en" { "Tool" } else { "工具" }.into(),
        CommandSource::Plugin => if language == "en" { "Plugin" } else { "插件" }.into(),
        CommandSource::Clipboard => if language == "en" { "Clip" } else { "剪贴" }.into(),
        CommandSource::History => if language == "en" {
            "History"
        } else {
            "历史"
        }
        .into(),
        CommandSource::Setting => if language == "en" {
            "Setting"
        } else {
            "设置"
        }
        .into(),
    }
}

/// 将搜索来源本地化成短标签。
pub(super) fn source_label(
    source: &devtools_core::search::SearchSource,
    language: &str,
) -> SharedString {
    match source {
        devtools_core::search::SearchSource::BuiltInTool => {
            if language == "en" { "Tool" } else { "工具" }.into()
        }
        devtools_core::search::SearchSource::Clipboard => {
            if language == "en" { "Clip" } else { "剪贴" }.into()
        }
        devtools_core::search::SearchSource::Plugin => {
            if language == "en" { "Plugin" } else { "插件" }.into()
        }
        devtools_core::search::SearchSource::History => if language == "en" {
            "History"
        } else {
            "历史"
        }
        .into(),
        devtools_core::search::SearchSource::Setting => if language == "en" {
            "Setting"
        } else {
            "设置"
        }
        .into(),
    }
}

/// 根据结果 ID 尝试读取本地化标题，找不到时使用搜索结果原文。
pub(super) fn localized_title(result_id: &str, language: &str, fallback: &str) -> String {
    if result_id.starts_with("tool.") || result_id.starts_with("setting.") {
        devtools_tools::localized_title_for(result_id, language)
    } else {
        fallback.to_string()
    }
}

/// 根据结果 ID 尝试读取本地化副标题。
pub(super) fn localized_subtitle(result_id: &str, language: &str, fallback: &str) -> String {
    if result_id.starts_with("tool.") || result_id.starts_with("setting.") {
        devtools_tools::localized_subtitle_for(result_id, language)
    } else {
        fallback.to_string()
    }
}

/// 压缩长文本摘要，避免剪贴板列表行被超长内容撑开。
pub(super) fn summarize(content: &str) -> String {
    let single_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() > 72 {
        format!("{}...", single_line.chars().take(72).collect::<String>())
    } else {
        single_line
    }
}
