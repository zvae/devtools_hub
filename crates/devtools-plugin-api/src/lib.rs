use serde::{Deserialize, Serialize};

pub type PluginId = String;
pub type CommandId = String;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PluginRuntime {
    BuiltIn,
    Wasm,
    Process,
    Native,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginPermissions {
    pub clipboard_read: bool,
    pub clipboard_write: bool,
    pub filesystem_read: bool,
    pub filesystem_write: bool,
    pub network_request: bool,
    pub shell_execute: bool,
    pub notification_show: bool,
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
