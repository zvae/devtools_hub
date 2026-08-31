use devtools_plugin_api::{CommandId, CommandResult};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginHostError {
    #[error("plugin runtime is not implemented in phase 0")]
    RuntimeUnavailable,
}

pub trait PluginHost {
    fn execute(
        &self,
        command_id: &CommandId,
        input: &str,
    ) -> Result<CommandResult, PluginHostError>;
}

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
