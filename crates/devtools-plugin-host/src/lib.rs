use devtools_plugin_api::{CommandId, CommandResult};
use thiserror::Error;

/// 插件宿主错误。阶段 0 只保留协议占位，真实插件运行时后续补齐。
#[derive(Debug, Error)]
pub enum PluginHostError {
    #[error("plugin runtime is not implemented in phase 0")]
    RuntimeUnavailable,
}

/// 插件宿主抽象：应用只依赖这个 trait，避免 UI/核心逻辑绑定具体运行时。
pub trait PluginHost {
    fn execute(
        &self,
        command_id: &CommandId,
        input: &str,
    ) -> Result<CommandResult, PluginHostError>;
}

/// 阶段 0 的占位宿主，明确返回未实现，方便先打通主流程。
#[derive(Default)]
pub struct PhaseZeroPluginHost;

impl PluginHost for PhaseZeroPluginHost {
    fn execute(
        &self,
        _command_id: &CommandId,
        _input: &str,
    ) -> Result<CommandResult, PluginHostError> {
        Err(PluginHostError::RuntimeUnavailable)
    }
}
