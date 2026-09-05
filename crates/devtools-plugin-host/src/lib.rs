use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::mpsc,
    time::Duration,
};

use devtools_plugin_api::{
    CommandDescriptor, CommandId, CommandResult, PluginExecutionContext, PluginExecutionRequest,
    PluginExecutionResponse, PluginManifest, PluginPermissions,
};
use thiserror::Error;
use wasmtime::{Config, Engine, Instance, Linker, Memory, Module, Store, TypedFunc};

const MAX_INPUT_BYTES: usize = 512 * 1024;
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(2);
const EXECUTION_FUEL: u64 = 10_000_000;

/// A successfully scanned plugin and its location on disk.
#[derive(Clone, Debug)]
pub struct RegisteredPlugin {
    pub manifest: PluginManifest,
    pub directory: PathBuf,
}

/// Scan results retain broken plugins as diagnostics rather than failing application startup.
#[derive(Clone, Debug, Default)]
pub struct PluginRegistry {
    plugins: BTreeMap<String, RegisteredPlugin>,
    command_plugins: BTreeMap<CommandId, String>,
    diagnostics: Vec<String>,
}

impl PluginRegistry {
    pub fn scan(plugin_roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut registry = Self::default();
        let mut seen_directories = BTreeSet::new();

        for root in plugin_roots {
            let canonical_root = root.canonicalize().unwrap_or(root);
            if !seen_directories.insert(canonical_root.clone()) {
                continue;
            }
            let Ok(entries) = fs::read_dir(&canonical_root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                registry.register_directory(path);
            }
        }
        registry
    }

    pub fn commands(&self) -> Vec<CommandDescriptor> {
        self.plugins
            .values()
            .flat_map(|plugin| plugin.manifest.command_descriptors())
            .collect()
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    pub fn plugin_for_command(&self, command_id: &str) -> Option<&RegisteredPlugin> {
        self.command_plugins
            .get(command_id)
            .and_then(|plugin_id| self.plugins.get(plugin_id))
    }

    pub fn plugins(&self) -> impl Iterator<Item = &RegisteredPlugin> {
        self.plugins.values()
    }

    fn register_directory(&mut self, directory: PathBuf) {
        let manifest_path = directory.join("plugin.toml");
        let manifest_text = match fs::read_to_string(&manifest_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                self.diagnostics.push(format!(
                    "could not read {}: {error}",
                    manifest_path.display()
                ));
                return;
            }
        };
        let manifest = match toml::from_str::<PluginManifest>(&manifest_text) {
            Ok(manifest) => manifest,
            Err(error) => {
                self.diagnostics.push(format!(
                    "could not parse {}: {error}",
                    manifest_path.display()
                ));
                return;
            }
        };
        if let Err(error) = manifest.validate() {
            self.diagnostics.push(format!(
                "invalid plugin manifest {}: {error}",
                manifest_path.display()
            ));
            return;
        }
        let entry_path = directory.join(&manifest.plugin.entry);
        if !entry_path.is_file() {
            self.diagnostics.push(format!(
                "plugin {} is missing entry file {}",
                manifest.plugin.id,
                entry_path.display()
            ));
            return;
        }
        if self.plugins.contains_key(&manifest.plugin.id) {
            self.diagnostics
                .push(format!("duplicate plugin id: {}", manifest.plugin.id));
            return;
        }
        if let Some(command) = manifest
            .commands
            .iter()
            .find(|command| self.command_plugins.contains_key(&command.id))
        {
            self.diagnostics
                .push(format!("duplicate plugin command id: {}", command.id));
            return;
        }

        let plugin_id = manifest.plugin.id.clone();
        for command in &manifest.commands {
            self.command_plugins
                .insert(command.id.clone(), plugin_id.clone());
        }
        self.plugins.insert(
            plugin_id,
            RegisteredPlugin {
                manifest,
                directory,
            },
        );
    }
}

#[derive(Debug, Error)]
pub enum PluginHostError {
    #[error("plugin command is not registered: {0}")]
    UnknownCommand(String),
    #[error("plugin {plugin_id} requires ungranted permissions")]
    PermissionDenied { plugin_id: String },
    #[error("plugin input exceeds the {MAX_INPUT_BYTES} byte limit")]
    InputTooLarge,
    #[error("plugin execution timed out after {} seconds", EXECUTION_TIMEOUT.as_secs())]
    TimedOut,
    #[error("plugin execution failed: {0}")]
    Execution(String),
}

/// The host boundary used by Core. No UI or system service is exposed to WASM by default.
pub trait PluginHost: Send + Sync {
    fn has_command(&self, command_id: &CommandId) -> bool;
    fn execute(
        &self,
        command_id: &CommandId,
        input: &str,
    ) -> Result<CommandResult, PluginHostError>;
}

/// WASM plugins use a small, allocation-based ABI so the plugin interface is portable.
///
/// Required exports: `memory`, `alloc(i32) -> i32`, and `execute(i32, i32) -> i64`.
/// `execute` returns `(result_ptr << 32) | result_len`; the result bytes are UTF-8 JSON.
pub struct WasmPluginHost {
    registry: PluginRegistry,
    granted_permissions: PluginPermissions,
    engine: Engine,
}

impl WasmPluginHost {
    pub fn new(
        registry: PluginRegistry,
        granted_permissions: PluginPermissions,
    ) -> Result<Self, PluginHostError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine =
            Engine::new(&config).map_err(|error| PluginHostError::Execution(error.to_string()))?;
        Ok(Self {
            registry,
            granted_permissions,
            engine,
        })
    }

    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }
}

impl PluginHost for WasmPluginHost {
    fn has_command(&self, command_id: &CommandId) -> bool {
        self.registry.plugin_for_command(command_id).is_some()
    }

    fn execute(
        &self,
        command_id: &CommandId,
        input: &str,
    ) -> Result<CommandResult, PluginHostError> {
        let plugin = self
            .registry
            .plugin_for_command(command_id)
            .ok_or_else(|| PluginHostError::UnknownCommand(command_id.clone()))?;
        if !self
            .granted_permissions
            .allows(&plugin.manifest.permissions)
        {
            return Err(PluginHostError::PermissionDenied {
                plugin_id: plugin.manifest.plugin.id.clone(),
            });
        }
        if input.len() > MAX_INPUT_BYTES {
            return Err(PluginHostError::InputTooLarge);
        }

        let request = PluginExecutionRequest {
            command: command_id.clone(),
            input: input.into(),
            context: PluginExecutionContext::default(),
        };
        let request = serde_json::to_vec(&request)
            .map_err(|error| PluginHostError::Execution(error.to_string()))?;
        let entry = plugin.directory.join(&plugin.manifest.plugin.entry);
        let engine = self.engine.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = sender.send(execute_wasm(engine, entry, request));
        });

        receiver
            .recv_timeout(EXECUTION_TIMEOUT)
            .map_err(|_| PluginHostError::TimedOut)?
    }
}

fn execute_wasm(
    engine: Engine,
    entry: PathBuf,
    request: Vec<u8>,
) -> Result<CommandResult, PluginHostError> {
    let module = Module::from_file(&engine, entry)
        .map_err(|error| PluginHostError::Execution(error.to_string()))?;
    let mut store = Store::new(&engine, ());
    store
        .set_fuel(EXECUTION_FUEL)
        .map_err(|error| PluginHostError::Execution(error.to_string()))?;
    let linker = Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|error| PluginHostError::Execution(error.to_string()))?;
    let memory = required_memory(&mut store, &instance)?;
    let alloc: TypedFunc<i32, i32> = required_func(&mut store, &instance, "alloc")?;
    let execute: TypedFunc<(i32, i32), i64> = required_func(&mut store, &instance, "execute")?;

    let input_ptr = alloc
        .call(&mut store, request.len() as i32)
        .map_err(|error| PluginHostError::Execution(error.to_string()))?;
    memory
        .write(&mut store, input_ptr as usize, &request)
        .map_err(|error| PluginHostError::Execution(error.to_string()))?;
    let packed_result = execute
        .call(&mut store, (input_ptr, request.len() as i32))
        .map_err(|error| PluginHostError::Execution(error.to_string()))?;
    let result_ptr = (packed_result >> 32) as u32 as usize;
    let result_len = packed_result as u32 as usize;
    if result_len > MAX_OUTPUT_BYTES {
        return Err(PluginHostError::Execution(
            "plugin result exceeds size limit".into(),
        ));
    }
    let mut result = vec![0; result_len];
    memory
        .read(&store, result_ptr, &mut result)
        .map_err(|error| PluginHostError::Execution(error.to_string()))?;
    let response: PluginExecutionResponse = serde_json::from_slice(&result)
        .map_err(|error| PluginHostError::Execution(format!("invalid JSON result: {error}")))?;
    Ok(response.into())
}

fn required_memory(store: &mut Store<()>, instance: &Instance) -> Result<Memory, PluginHostError> {
    instance
        .get_memory(store, "memory")
        .ok_or_else(|| PluginHostError::Execution("missing required `memory` export".into()))
}

fn required_func<Params, Results>(
    store: &mut Store<()>,
    instance: &Instance,
    name: &str,
) -> Result<TypedFunc<Params, Results>, PluginHostError>
where
    Params: wasmtime::WasmParams,
    Results: wasmtime::WasmResults,
{
    instance
        .get_typed_func(store, name)
        .map_err(|error| PluginHostError::Execution(format!("invalid `{name}` export: {error}")))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::PluginRegistry;

    #[test]
    fn scanning_ignores_a_plugin_without_its_entry_file() {
        let directory = std::env::temp_dir().join(format!(
            "devtools-hub-plugin-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let plugin_dir = directory.join("example");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
            [plugin]
            id = "devtools.example"
            name = "Example"
            version = "0.1.0"
            runtime = "wasm"
            entry = "plugin.wasm"
            [[commands]]
            id = "example.run"
            title = "Run example"
        "#,
        )
        .unwrap();

        let registry = PluginRegistry::scan([directory.clone()]);
        assert!(registry.commands().is_empty());
        assert_eq!(registry.diagnostics().len(), 1);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn scanning_registers_commands_for_a_complete_plugin_directory() {
        let directory = std::env::temp_dir().join(format!(
            "devtools-hub-plugin-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let plugin_dir = directory.join("example");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("plugin.wasm"), []).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
            [plugin]
            id = "devtools.example"
            name = "Example"
            version = "0.1.0"
            runtime = "wasm"
            entry = "plugin.wasm"
            [[commands]]
            id = "example.run"
            title = "Run example"
            keywords = ["example"]
        "#,
        )
        .unwrap();

        let registry = PluginRegistry::scan([directory.clone()]);
        assert_eq!(registry.commands().len(), 1);
        assert!(registry.plugin_for_command("example.run").is_some());
        let _ = fs::remove_dir_all(directory);
    }
}
