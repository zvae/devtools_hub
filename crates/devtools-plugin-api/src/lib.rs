use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type PluginId = String;
pub type CommandId = String;

/// Runtime declared by a plugin manifest. Phase 2 only executes WASM plugins.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginRuntime {
    BuiltIn,
    Wasm,
    Process,
    Native,
}

/// Permissions requested by a plugin. They are requests, not grants.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginPermissions {
    #[serde(default, alias = "clipboard")]
    pub clipboard_read: bool,
    #[serde(default)]
    pub clipboard_write: bool,
    #[serde(default, alias = "filesystem")]
    pub filesystem_read: bool,
    #[serde(default)]
    pub filesystem_write: bool,
    #[serde(default, alias = "network")]
    pub network_request: bool,
    #[serde(default, alias = "shell")]
    pub shell_execute: bool,
    #[serde(default, alias = "notification")]
    pub notification_show: bool,
}

impl PluginPermissions {
    pub fn allows(&self, requested: &Self) -> bool {
        (!requested.clipboard_read || self.clipboard_read)
            && (!requested.clipboard_write || self.clipboard_write)
            && (!requested.filesystem_read || self.filesystem_read)
            && (!requested.filesystem_write || self.filesystem_write)
            && (!requested.network_request || self.network_request)
            && (!requested.shell_execute || self.shell_execute)
            && (!requested.notification_show || self.notification_show)
    }

    pub fn is_empty(&self) -> bool {
        !self.clipboard_read
            && !self.clipboard_write
            && !self.filesystem_read
            && !self.filesystem_write
            && !self.network_request
            && !self.shell_execute
            && !self.notification_show
    }
}

/// plugin.toml root model. File paths are intentionally kept relative to the plugin directory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginManifest {
    pub plugin: PluginMetadata,
    #[serde(default)]
    pub commands: Vec<PluginCommand>,
    #[serde(default)]
    pub permissions: PluginPermissions,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginMetadata {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    pub runtime: PluginRuntime,
    pub entry: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginCommand {
    pub id: CommandId,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub input: PluginInput,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginInput {
    #[default]
    None,
    Text,
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !is_identifier(&self.plugin.id) {
            return Err(ManifestError::InvalidPluginId(self.plugin.id.clone()));
        }
        if self.plugin.name.trim().is_empty() || self.plugin.version.trim().is_empty() {
            return Err(ManifestError::MissingMetadata);
        }
        let entry = Path::new(&self.plugin.entry);
        if self.plugin.entry.trim().is_empty()
            || entry.is_absolute()
            || entry.components().count() != 1
        {
            return Err(ManifestError::UnsafeEntry(self.plugin.entry.clone()));
        }
        if self.plugin.runtime != PluginRuntime::Wasm {
            return Err(ManifestError::UnsupportedRuntime(
                self.plugin.runtime.clone(),
            ));
        }
        if self.commands.is_empty() {
            return Err(ManifestError::NoCommands);
        }
        let mut command_ids = std::collections::BTreeSet::new();
        for command in &self.commands {
            if !is_identifier(&command.id) || command.title.trim().is_empty() {
                return Err(ManifestError::InvalidCommandId(command.id.clone()));
            }
            if !command_ids.insert(&command.id) {
                return Err(ManifestError::DuplicateCommandId(command.id.clone()));
            }
        }
        Ok(())
    }

    pub fn command_descriptors(&self) -> Vec<CommandDescriptor> {
        self.commands
            .iter()
            .map(|command| CommandDescriptor {
                id: command.id.clone(),
                plugin_id: Some(self.plugin.id.clone()),
                title: command.title.clone(),
                subtitle: if command.subtitle.is_empty() {
                    self.plugin.description.clone()
                } else {
                    command.subtitle.clone()
                },
                i18n: Vec::new(),
                keywords: command.keywords.clone(),
                source: CommandSource::Plugin,
            })
            .collect()
    }
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum ManifestError {
    #[error("plugin id must contain only ASCII letters, digits, '.', '_' or '-': {0}")]
    InvalidPluginId(String),
    #[error("plugin metadata must include a name and version")]
    MissingMetadata,
    #[error("plugin entry must be a file name inside the plugin directory: {0}")]
    UnsafeEntry(String),
    #[error("plugin runtime is not supported in phase 2: {0:?}")]
    UnsupportedRuntime(PluginRuntime),
    #[error("plugin must declare at least one command")]
    NoCommands,
    #[error("command id or title is invalid: {0}")]
    InvalidCommandId(String),
    #[error("duplicate command id: {0}")]
    DuplicateCommandId(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandDescriptor {
    pub id: CommandId,
    pub plugin_id: Option<PluginId>,
    pub title: String,
    pub subtitle: String,
    pub i18n: Vec<CommandI18n>,
    pub keywords: Vec<String>,
    pub source: CommandSource,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandI18n {
    pub locale: String,
    pub title: String,
    pub subtitle: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CommandSource {
    BuiltInTool,
    Plugin,
    Clipboard,
    History,
    Setting,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CommandAction {
    OpenTool { tool_id: String },
    CopyText { text: String },
    ShowText { title: String, content: String },
    Noop,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandResult {
    pub title: String,
    pub content: String,
}

/// JSON payload passed to a WASM plugin's `execute` export.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginExecutionRequest {
    pub command: CommandId,
    pub input: String,
    pub context: PluginExecutionContext,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginExecutionContext {
    pub theme: String,
    pub platform: String,
    pub locale: String,
}

impl Default for PluginExecutionContext {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            platform: std::env::consts::OS.into(),
            locale: "zh-CN".into(),
        }
    }
}

/// JSON output returned by a WASM plugin. UI actions remain structured for future rendering.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginExecutionResponse {
    #[serde(rename = "type")]
    pub content_type: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub actions: Vec<PluginResultAction>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginResultAction {
    pub id: String,
    pub title: String,
    pub kind: String,
}

impl From<PluginExecutionResponse> for CommandResult {
    fn from(value: PluginExecutionResponse) -> Self {
        Self {
            title: value.title,
            content: value.content,
        }
    }
}
