use anyhow::Result;
use devtools_storage::{hash_text, Storage};
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
    },
    ToolCompleted {
        title: String,
        content: String,
    },
    CommandExecuted {
        title: String,
        content: String,
    },
    SettingsLoaded {
        theme: String,
        language: String,
        hotkey: String,
        autostart: bool,
        middle_click_enabled: bool,
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
    CopyRequested {
        text: String,
    },
    ToggleWindow,
    ExitRequested,
    Error(String),
}

/// 应用运行时负责任务调度、持久化调用和搜索结果生成，不直接操作 UI。
pub struct AppRuntime {
    storage: Storage,
    search: SearchEngine,
    requests: mpsc::UnboundedReceiver<AppRequest>,
    events: mpsc::UnboundedSender<AppEvent>,
}

impl AppRuntime {
    /// 创建核心运行时，并用内置工具初始化搜索引擎。
    pub fn new(
        storage: Storage,
        requests: mpsc::UnboundedReceiver<AppRequest>,
        events: mpsc::UnboundedSender<AppEvent>,
    ) -> Self {
        Self {
            storage,
            search: SearchEngine::new(devtools_tools::builtin_commands()),
            requests,
            events,
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
            AppRequest::ClearClipboard => {
                self.storage.clear_clipboard()?;
                self.events.send(AppEvent::CommandExecuted {
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
    fn activate_result(&self, result_id: &str) -> Result<()> {
        if result_id == "setting.theme" {
            return self.load_settings();
        }

        if result_id == "tool.uuid.v4" {
            self.storage.record_execution(result_id, None)?;
            let result = devtools_tools::execute(result_id, "");
            self.events.send(AppEvent::CopyRequested {
                text: result.content.clone(),
            })?;
            self.events.send(AppEvent::CommandExecuted {
                title: result.title,
                content: result.content,
            })?;
            return Ok(());
        }

        if result_id.starts_with("tool.") {
            self.storage.record_execution(result_id, None)?;
            self.events.send(AppEvent::ToolOpened {
                tool_id: result_id.to_string(),
                title: devtools_tools::title_for(result_id),
            })?;
            return Ok(());
        }

        self.events.send(AppEvent::CommandExecuted {
            title: "Command".into(),
            content: format!("No handler for {result_id}"),
        })?;
        Ok(())
    }

    /// 执行工具并记录输入摘要，避免把原始输入重复写入执行历史。
    fn run_tool(&self, tool_id: &str, input: &str) -> Result<()> {
        self.storage
            .record_execution(tool_id, Some(&hash_text(input)))?;
        let result = devtools_tools::execute(tool_id, input);
        self.events.send(AppEvent::ToolCompleted {
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
        self.events.send(AppEvent::SettingsLoaded {
            theme,
            language,
            hotkey,
            autostart,
            middle_click_enabled,
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
        let mut results = self.search.search_commands(query, 12);
        results.sort_by(|left, right| right.score.total_cmp(&left.score));
        results.truncate(12);
        Ok(results)
    }
}
