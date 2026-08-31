use anyhow::Result;
use devtools_storage::{hash_text, Storage};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::search::{SearchEngine, SearchResult};

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
    ClearClipboard,
    ToggleWindow,
    Exit,
}

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
    CopyRequested {
        text: String,
    },
    ToggleWindow,
    ExitRequested,
    Error(String),
}

pub struct AppRuntime {
    storage: Storage,
    search: SearchEngine,
    requests: mpsc::UnboundedReceiver<AppRequest>,
    events: mpsc::UnboundedSender<AppEvent>,
}

impl AppRuntime {
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

    pub async fn run(mut self) {
        while let Some(request) = self.requests.recv().await {
            if let Err(error) = self.handle_request(request).await {
                warn!(?error, "runtime request failed");
                let _ = self.events.send(AppEvent::Error(error.to_string()));
            }
        }
    }

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
        self.events.send(AppEvent::SettingsLoaded {
            theme,
            language,
            hotkey,
        })?;
        Ok(())
    }

    fn load_theme(&self) -> Result<()> {
        let theme = self
            .storage
            .get_setting("theme")?
            .unwrap_or_else(|| "dark".into());
        self.events.send(AppEvent::ThemeChanged { theme })?;
        Ok(())
    }

    fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let mut results = self.search.search_commands(query, 12);
        results.sort_by(|left, right| right.score.total_cmp(&left.score));
        results.truncate(12);
        Ok(results)
    }
}
