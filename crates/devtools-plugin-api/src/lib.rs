use serde::{Deserialize, Serialize};

/// 插件 ID 与命令 ID 使用字符串，便于跨进程、配置文件和未来插件清单复用。
pub type PluginId = String;
pub type CommandId = String;

/// 插件运行时类型：当前阶段先声明协议，具体执行能力由插件宿主逐步实现。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PluginRuntime {
    BuiltIn,
    Wasm,
    Process,
    Native,
}

/// 插件权限声明，用于后续安装审核、运行时授权和 UI 提示。
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

/// 命令描述符是搜索、工具列表、插件扩展之间共享的最小命令模型。
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

/// 命令本地化文本。默认 title/subtitle 作为英文兜底，其它语言放在 i18n 中。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandI18n {
    pub locale: String,
    pub title: String,
    pub subtitle: String,
}

/// 命令来源用于 UI 标签展示、搜索分组以及后续权限隔离。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CommandSource {
    BuiltInTool,
    Plugin,
    Clipboard,
    History,
    Setting,
}

/// 命令激活后的动作协议，内置工具和插件都可以映射到这些基础动作。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CommandAction {
    OpenTool { tool_id: String },
    CopyText { text: String },
    ShowText { title: String, content: String },
    Noop,
}

/// 命令执行结果统一返回标题和文本内容，便于工具窗口与通知复用。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandResult {
    pub title: String,
    pub content: String,
}
