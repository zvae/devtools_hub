//! 通用工具运行时：校验、按需实例化、配置合并；不依赖任何具体工具。

use devtools_module_api::{ModuleDescriptor, ModuleSettings, Selection, ToolEffect, ToolModule};
use devtools_ui::style::Palette;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub struct ModuleRuntime {
    descriptors: Vec<ModuleDescriptor>,
    instances: BTreeMap<&'static str, Box<dyn ToolModule>>,
    read_only: BTreeSet<&'static str>,
}

impl ModuleRuntime {
    pub fn new(mut descriptors: Vec<ModuleDescriptor>) -> Result<Self, String> {
        let mut ids = BTreeSet::new();
        for d in &descriptors {
            if d.id.is_empty() || !ids.insert(d.id) {
                return Err(format!("工具 ID 为空或重复：{}", d.id));
            }
            if d.size.iter().any(|x| !x.is_finite() || *x < 120.0) {
                return Err(format!("工具尺寸无效：{}", d.id));
            }
        }
        descriptors.sort_by_key(|d| (d.order, d.id));
        Ok(Self {
            descriptors,
            instances: Default::default(),
            read_only: Default::default(),
        })
    }

    pub fn descriptor(&self, id: &str) -> Option<&ModuleDescriptor> {
        self.descriptors.iter().find(|d| d.id == id)
    }
    pub fn instantiated(&self) -> usize {
        self.instances.len()
    }
    pub fn writable(&self, id: &str) -> bool {
        !self.read_only.contains(id)
    }

    fn ensure(&mut self, id: &str, settings: &ModuleSettings) -> Result<&'static str, String> {
        let d = *self
            .descriptor(id)
            .ok_or_else(|| format!("工具不存在：{id}"))?;
        if !settings.enabled {
            return Err(format!("工具已禁用：{}", d.name));
        }
        if !self.instances.contains_key(d.id) {
            let valid = settings.version == 1 && (d.validate_config)(&settings.settings);
            if !valid {
                self.read_only.insert(d.id);
            }
            let config = if valid {
                settings.settings.clone()
            } else {
                Value::Object(Default::default())
            };
            self.instances.insert(d.id, (d.create)(&config));
        }
        Ok(d.id)
    }

    pub fn activate(
        &mut self,
        id: &str,
        input: Option<Selection>,
        settings: &ModuleSettings,
    ) -> Result<Vec<ToolEffect>, String> {
        let id = self.ensure(id, settings)?;
        let mut effects = self.instances.get_mut(id).unwrap().activate(input);
        if self.read_only.contains(id) {
            effects.push(ToolEffect::Toast(
                "工具配置无法读取，使用默认值且保留原配置".into(),
            ));
        }
        Ok(effects)
    }

    pub fn render(&mut self, id: &str, ui: &mut egui::Ui, palette: &Palette) -> Vec<ToolEffect> {
        self.instances
            .get_mut(id)
            .map_or_else(Vec::new, |tool| tool.render(ui, palette))
    }
    pub fn pinned(&self, id: &str) -> bool {
        self.instances.get(id).is_some_and(|t| t.pinned())
    }
    pub fn on_hidden(&mut self, id: &str) {
        if let Some(t) = self.instances.get_mut(id) {
            t.on_hidden();
        }
    }
    pub fn discard(&mut self, id: &str) {
        self.instances.remove(id);
        self.read_only.remove(id);
    }
    pub fn save_config(&self, id: &str, settings: &mut ModuleSettings) -> bool {
        if self.read_only.contains(id) {
            return false;
        }
        let Some(tool) = self.instances.get(id) else {
            return false;
        };
        let before = settings.settings.clone();
        merge(&mut settings.settings, tool.config());
        before != settings.settings
    }
    pub fn settings_ui(
        &mut self,
        id: &str,
        ui: &mut egui::Ui,
        settings: &ModuleSettings,
    ) -> Result<(), String> {
        let id = self.ensure(id, settings)?;
        if self.read_only.contains(id) {
            ui.label("配置只读；原始值已保留");
        } else {
            self.instances.get_mut(id).unwrap().settings_ui(ui);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Counter(u32);
    impl ToolModule for Counter {
        fn activate(&mut self, _: Option<Selection>) -> Vec<ToolEffect> {
            self.0 += 1;
            vec![ToolEffect::Toast(self.0.to_string())]
        }
        fn render(&mut self, _: &mut egui::Ui, _: &Palette) -> Vec<ToolEffect> {
            Vec::new()
        }
        fn pinned(&self) -> bool {
            false
        }
        fn config(&self) -> Value {
            serde_json::json!({"known": self.0})
        }
    }
    fn descriptor() -> ModuleDescriptor {
        ModuleDescriptor {
            id: "test.counter",
            name: "计数",
            description: "",
            order: 0,
            size: [300.0, 200.0],
            toast_offset: -48.0,
            matches: |_| true,
            paint_icon: |_, _, _| {},
            create: |_| Box::new(Counter(0)),
            validate_config: Value::is_object,
            default_hotkey: "",
        }
    }
    #[test]
    fn instances_are_lazy_and_isolated_between_apps() {
        let mut a = ModuleRuntime::new(vec![descriptor()]).unwrap();
        let mut b = ModuleRuntime::new(vec![descriptor()]).unwrap();
        assert_eq!(a.instantiated(), 0);
        let config = ModuleSettings::default();
        assert_eq!(
            a.activate("test.counter", None, &config).unwrap(),
            vec![ToolEffect::Toast("1".into())]
        );
        assert_eq!(
            a.activate("test.counter", None, &config).unwrap(),
            vec![ToolEffect::Toast("2".into())]
        );
        assert_eq!(
            b.activate("test.counter", None, &config).unwrap(),
            vec![ToolEffect::Toast("1".into())]
        );
    }
    #[test]
    fn config_merge_preserves_unknown_fields_and_protects_future_versions() {
        let mut runtime = ModuleRuntime::new(vec![descriptor()]).unwrap();
        let mut config = ModuleSettings {
            settings: serde_json::json!({"unknown":{"keep":true}}),
            ..Default::default()
        };
        runtime.activate("test.counter", None, &config).unwrap();
        assert!(runtime.save_config("test.counter", &mut config));
        assert_eq!(config.settings["unknown"]["keep"], true);
        runtime.discard("test.counter");
        config.version = 99;
        let original = config.clone();
        runtime.activate("test.counter", None, &config).unwrap();
        assert!(!runtime.save_config("test.counter", &mut config));
        assert_eq!(config, original);
    }
    #[test]
    fn disabled_missing_and_duplicate_tools_are_rejected() {
        assert!(ModuleRuntime::new(vec![descriptor(), descriptor()]).is_err());
        let mut runtime = ModuleRuntime::new(vec![descriptor()]).unwrap();
        assert!(runtime
            .activate("missing", None, &Default::default())
            .is_err());
        assert!(runtime
            .activate(
                "test.counter",
                None,
                &ModuleSettings {
                    enabled: false,
                    ..Default::default()
                }
            )
            .is_err());
        assert_eq!(runtime.instantiated(), 0);
    }
}

fn merge(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                merge(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, patch) => *target = patch,
    }
}
