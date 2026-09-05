use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use devtools_plugin_api::CommandDescriptor;
use devtools_plugin_host::PluginHost;
use devtools_storage::{hash_text, Storage, ToolHistoryRecord};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::search::{SearchEngine, SearchResult};

/// UI、托盘、快捷键、剪贴板等前端入口发送给核心运行时的请求。
#[derive(Clone, Debug)]
pub enum AppRequest {
    Search {
        query: String,
    },
    ClipboardChanged {
        content: String,
        source_app: Option<String>,
    },
    ActivateResult {
        result_id: String,
    },
    RunTool {
        tool_id: String,
        input: String,
    },
    CommitToolHistory {
        tool_id: String,
        input: String,
        output: String,
        revision: u64,
    },
    OpenSettings,
    LoadTheme,
    SetTheme {
        theme: String,
    },
    SetLanguage {
        language: String,
    },
    SetHotkey {
        hotkey: String,
    },
    SetAutostart {
        enabled: bool,
    },
    SetMiddleClickEnabled {
        enabled: bool,
    },
    SetToolHistoryLimit {
        limit: usize,
    },
    SetSingleToolWindow {
        enabled: bool,
    },
    TimestampConvertUnix {
        input: String,
    },
    TimestampConvertDatetime {
        input: String,
        milliseconds: bool,
    },
    TimestampConvertParts {
        year: String,
        month: String,
        day: String,
        hour: String,
        minute: String,
        second: String,
        milliseconds: bool,
    },
    ClearClipboard,
    ToggleWindow,
    Exit,
}

/// 核心运行时处理完成后发回 UI 线程的事件。
#[derive(Clone, Debug)]
pub enum AppEvent {
    SearchCompleted(Vec<SearchResult>),
    ClipboardStored,
    ToolOpened {
        tool_id: String,
        title: String,
        history: Vec<ToolHistoryRecord>,
    },
    ToolCompleted {
        tool_id: String,
        title: String,
        content: String,
    },
    ToolHistoryUpdated {
        tool_id: String,
        history: Vec<ToolHistoryRecord>,
    },
    CommandExecuted {
        command_id: Option<String>,
        title: String,
        content: String,
    },
    SettingsLoaded {
        theme: String,
        language: String,
        hotkey: String,
        autostart: bool,
        middle_click_enabled: bool,
        tool_history_limit: usize,
        single_tool_window: bool,
    },
    ThemeChanged {
        theme: String,
    },
    LanguageChanged {
        language: String,
    },
    HotkeyChanged {
        hotkey: String,
    },
    AutostartChanged {
        enabled: bool,
    },
    MiddleClickChanged {
        enabled: bool,
    },
    ToolHistoryLimitChanged {
        limit: usize,
    },
    SingleToolWindowChanged {
        enabled: bool,
    },
    TimestampConverted {
        mode: TimestampConversionMode,
        value: String,
    },
    CopyRequested {
        text: String,
    },
    ToggleWindow,
    ExitRequested,
    Error(String),
}

#[derive(Clone, Copy, Debug)]
pub enum TimestampConversionMode {
    UnixToDatetime,
    DatetimeToUnix,
    PartsToUnix,
}

/// 应用运行时负责任务调度、持久化调用和搜索结果生成，不直接操作 UI。
pub struct AppRuntime {
    storage: Storage,
    search: SearchEngine,
    commands: Vec<CommandDescriptor>,
    plugin_host: Arc<dyn PluginHost>,
    usage: HashMap<String, ToolUsage>,
    usage_sequence: u64,
    requests: mpsc::UnboundedReceiver<AppRequest>,
    request_tx: mpsc::UnboundedSender<AppRequest>,
    events: mpsc::UnboundedSender<AppEvent>,
    history_revisions: HashMap<String, u64>,
}

impl AppRuntime {
    /// 创建核心运行时，并用内置工具初始化搜索引擎。
    pub fn new(
        storage: Storage,
        commands: Vec<CommandDescriptor>,
        plugin_host: Arc<dyn PluginHost>,
        request_tx: mpsc::UnboundedSender<AppRequest>,
        requests: mpsc::UnboundedReceiver<AppRequest>,
        events: mpsc::UnboundedSender<AppEvent>,
    ) -> Self {
        Self {
            storage,
            search: SearchEngine::new(commands.clone()),
            commands,
            plugin_host,
            usage: HashMap::new(),
            usage_sequence: 0,
            requests,
            request_tx,
            events,
            history_revisions: HashMap::new(),
        }
    }

    /// 持续消费请求通道，任何错误都会转成 AppEvent::Error 回传给 UI。
    pub async fn run(mut self) {
        while let Some(request) = self.requests.recv().await {
            if let Err(error) = self.handle_request(request).await {
                warn!(?error, "runtime request failed");
                let _ = self.events.send(AppEvent::Error(error.to_string()));
            }
        }
    }

    /// 将不同来源的请求分发到对应处理逻辑，保持 UI 回调尽量轻量。
    async fn handle_request(&mut self, request: AppRequest) -> Result<()> {
        match request {
            AppRequest::Search { query } => {
                let results = self.search(&query)?;
                self.events.send(AppEvent::SearchCompleted(results))?;
            }
            AppRequest::ClipboardChanged {
                content,
                source_app,
            } => {
                if self
                    .storage
                    .upsert_clipboard_text_from(&content, source_app.as_deref())?
                    .is_some()
                {
                    self.events.send(AppEvent::ClipboardStored)?;
                    let results = self.search("")?;
                    self.events.send(AppEvent::SearchCompleted(results))?;
                }
            }
            AppRequest::ActivateResult { result_id } => self.activate_result(&result_id)?,
            AppRequest::RunTool { tool_id, input } => self.run_tool(&tool_id, &input)?,
            AppRequest::CommitToolHistory {
                tool_id,
                input,
                output,
                revision,
            } => self.commit_tool_history(&tool_id, &input, &output, revision)?,
            AppRequest::OpenSettings => self.load_settings()?,
            AppRequest::LoadTheme => self.load_theme()?,
            AppRequest::SetTheme { theme } => {
                self.storage.set_setting("theme", &theme)?;
                self.events.send(AppEvent::ThemeChanged { theme })?;
            }
            AppRequest::SetLanguage { language } => {
                self.storage.set_setting("language", &language)?;
                self.events.send(AppEvent::LanguageChanged { language })?;
            }
            AppRequest::SetHotkey { hotkey } => {
                self.storage.set_setting("hotkey", &hotkey)?;
                self.events.send(AppEvent::HotkeyChanged { hotkey })?;
            }
            AppRequest::SetAutostart { enabled } => {
                self.storage
                    .set_setting("autostart", if enabled { "true" } else { "false" })?;
                self.events.send(AppEvent::AutostartChanged { enabled })?;
            }
            AppRequest::SetMiddleClickEnabled { enabled } => {
                self.storage.set_setting(
                    "middle_click_enabled",
                    if enabled { "true" } else { "false" },
                )?;
                self.events.send(AppEvent::MiddleClickChanged { enabled })?;
            }
            AppRequest::SetToolHistoryLimit { limit } => {
                let limit = limit.min(1_000);
                self.storage.set_tool_history_limit(limit)?;
                self.events
                    .send(AppEvent::ToolHistoryLimitChanged { limit })?;
            }
            AppRequest::SetSingleToolWindow { enabled } => {
                self.storage
                    .set_setting("single_tool_window", if enabled { "true" } else { "false" })?;
                self.events
                    .send(AppEvent::SingleToolWindowChanged { enabled })?;
            }
            AppRequest::TimestampConvertUnix { input } => {
                let result = input
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| "timestamp must be seconds".to_string())
                    .and_then(devtools_tools::timestamp_to_datetime_auto);
                self.convert_timestamp(TimestampConversionMode::UnixToDatetime, result)?;
            }
            AppRequest::TimestampConvertDatetime {
                input,
                milliseconds,
            } => {
                self.convert_timestamp(
                    TimestampConversionMode::DatetimeToUnix,
                    devtools_tools::timestamp_from_datetime_with_unit(&input, milliseconds),
                )?;
            }
            AppRequest::TimestampConvertParts {
                year,
                month,
                day,
                hour,
                minute,
                second,
                milliseconds,
            } => {
                self.convert_timestamp(
                    TimestampConversionMode::PartsToUnix,
                    devtools_tools::timestamp_from_parts_with_unit(
                        &year,
                        &month,
                        &day,
                        &hour,
                        &minute,
                        &second,
                        milliseconds,
                    ),
                )?;
            }
            AppRequest::ClearClipboard => {
                self.storage.clear_clipboard()?;
                self.events.send(AppEvent::CommandExecuted {
                    command_id: None,
                    title: "Clipboard".into(),
                    content: "Clipboard history cleared".into(),
                })?;
                self.events
                    .send(AppEvent::SearchCompleted(self.search("")?))?;
            }
            AppRequest::ToggleWindow => {
                debug!("shortcut toggle requested");
                self.events.send(AppEvent::ToggleWindow)?;
            }
            AppRequest::Exit => {
                self.events.send(AppEvent::ExitRequested)?;
            }
        }

        Ok(())
    }

    /// 激活搜索结果。可立即复制的命令直接执行，其它工具交给 UI 打开窗口。
    fn activate_result(&mut self, result_id: &str) -> Result<()> {
        if result_id == "setting.theme" {
            return self.load_settings();
        }

        if result_id == "tool.uuid.v4" {
            self.storage.record_execution(result_id, None)?;
            let result = devtools_tools::execute(result_id, "");
            self.record_usage(result_id);
            self.events.send(AppEvent::CopyRequested {
                text: result.content.clone(),
            })?;
            self.events.send(AppEvent::CommandExecuted {
                command_id: Some(result_id.to_string()),
                title: result.title,
                content: result.content,
            })?;
            return Ok(());
        }

        if self.command(result_id).is_some() {
            self.storage.record_execution(result_id, None)?;
            let history = self
                .storage
                .tool_history(result_id, self.tool_history_limit()?)?;
            self.events.send(AppEvent::ToolOpened {
                tool_id: result_id.to_string(),
                title: self
                    .command(result_id)
                    .map(|command| command.title.clone())
                    .unwrap_or_else(|| result_id.to_string()),
                history,
            })?;
            return Ok(());
        }

        self.events.send(AppEvent::CommandExecuted {
            command_id: None,
            title: "Command".into(),
            content: format!("No handler for {result_id}"),
        })?;
        Ok(())
    }

    /// 执行工具并记录输入摘要，避免把原始输入重复写入执行历史。
    fn run_tool(&mut self, tool_id: &str, input: &str) -> Result<()> {
        let result = if self.plugin_host.has_command(&tool_id.to_string()) {
            self.plugin_host.execute(&tool_id.to_string(), input)?
        } else {
            devtools_tools::execute(tool_id, input)
        };
        self.storage
            .record_execution(tool_id, Some(&hash_text(input)))?;
        self.record_usage(tool_id);
        self.schedule_tool_history(tool_id, input, &result.content);
        self.events.send(AppEvent::ToolCompleted {
            tool_id: tool_id.to_string(),
            title: result.title,
            content: result.content,
        })?;
        Ok(())
    }

    /// 读取设置页需要的完整配置。
    fn load_settings(&self) -> Result<()> {
        let theme = self
            .storage
            .get_setting("theme")?
            .unwrap_or_else(|| "dark".into());
        let language = self
            .storage
            .get_setting("language")?
            .unwrap_or_else(|| "zh-CN".into());
        let hotkey = self
            .storage
            .get_setting("hotkey")?
            .unwrap_or_else(|| "Alt+Space".into());
        let autostart = self
            .storage
            .get_setting("autostart")?
            .map(|value| value == "true" || value == "1")
            .unwrap_or(false);
        let middle_click_enabled = self
            .storage
            .get_setting("middle_click_enabled")?
            .map(|value| value == "true" || value == "1")
            .unwrap_or(true);
        let tool_history_limit = self.tool_history_limit()?;
        let single_tool_window = self
            .storage
            .get_setting("single_tool_window")?
            .map(|value| value == "true" || value == "1")
            .unwrap_or(true);
        self.events.send(AppEvent::SettingsLoaded {
            theme,
            language,
            hotkey,
            autostart,
            middle_click_enabled,
            tool_history_limit,
            single_tool_window,
        })?;
        Ok(())
    }

    /// 启动时只需要先加载主题，避免强制切到设置页。
    fn load_theme(&self) -> Result<()> {
        let theme = self
            .storage
            .get_setting("theme")?
            .unwrap_or_else(|| "dark".into());
        self.events.send(AppEvent::ThemeChanged { theme })?;
        Ok(())
    }

    /// 当前阶段先搜索命令，后续会合并剪贴板、历史和插件结果。
    fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let mut results = self
            .search
            .search_commands(query, self.commands.len().max(12));
        results.sort_by(|left, right| self.compare_usage(left, right));
        results.truncate(12);
        Ok(results)
    }

    fn command(&self, command_id: &str) -> Option<&CommandDescriptor> {
        self.commands
            .iter()
            .find(|command| command.id == command_id)
    }

    fn tool_history_limit(&self) -> Result<usize> {
        Ok(self
            .storage
            .get_setting("tool_history_limit")?
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(20)
            .min(1_000))
    }

    fn record_usage(&mut self, command_id: &str) {
        self.usage_sequence += 1;
        let usage = self.usage.entry(command_id.to_string()).or_default();
        usage.last_used = self.usage_sequence;
        usage.total += 1;
    }

    fn compare_usage(&self, left: &SearchResult, right: &SearchResult) -> std::cmp::Ordering {
        let left_usage = self.usage.get(&left.id);
        let right_usage = self.usage.get(&right.id);
        match (left_usage, right_usage) {
            (Some(left_usage), Some(right_usage)) => right_usage
                .last_used
                .cmp(&left_usage.last_used)
                .then_with(|| right_usage.total.cmp(&left_usage.total))
                .then_with(|| right.score.total_cmp(&left.score)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => right.score.total_cmp(&left.score),
        }
    }

    fn schedule_tool_history(&mut self, tool_id: &str, input: &str, output: &str) {
        let revision = self
            .history_revisions
            .entry(tool_id.to_string())
            .or_insert(0);
        *revision += 1;
        let request = AppRequest::CommitToolHistory {
            tool_id: tool_id.to_string(),
            input: input.to_string(),
            output: output.to_string(),
            revision: *revision,
        };
        let request_tx = self.request_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
            let _ = request_tx.send(request);
        });
    }

    fn commit_tool_history(
        &mut self,
        tool_id: &str,
        input: &str,
        output: &str,
        revision: u64,
    ) -> Result<()> {
        if self.history_revisions.get(tool_id).copied() != Some(revision) {
            return Ok(());
        }
        self.storage
            .append_tool_history(tool_id, input, output, self.tool_history_limit()?)?;
        let history = self
            .storage
            .tool_history(tool_id, self.tool_history_limit()?)?;
        self.events.send(AppEvent::ToolHistoryUpdated {
            tool_id: tool_id.to_string(),
            history,
        })?;
        Ok(())
    }

    fn convert_timestamp(
        &mut self,
        mode: TimestampConversionMode,
        result: std::result::Result<String, String>,
    ) -> Result<()> {
        let value = match result {
            Ok(value) => {
                self.storage
                    .record_execution("tool.timestamp.convert", None)?;
                self.record_usage("tool.timestamp.convert");
                value
            }
            Err(error) => format!("错误: {error}"),
        };
        self.events
            .send(AppEvent::TimestampConverted { mode, value })?;
        Ok(())
    }
}

#[derive(Default)]
struct ToolUsage {
    last_used: u64,
    total: u32,
}
