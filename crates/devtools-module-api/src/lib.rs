//! 编译期工具协议：声明、输入、窗口策略和绘制后执行的宿主请求。

use devtools_ui::style::Palette;
use egui::{Rect, Ui};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// 选区只携带数据，不包含任何具体工具类型。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Text(String),
    Files(Vec<String>),
}

impl Selection {
    pub fn label(&self) -> String {
        match self {
            Self::Text(text) => format!("选中的文本 {} 个", text.chars().count()),
            Self::Files(paths) => format!("选中的文件 {} 个", paths.len()),
        }
    }
}

/// 工具返回请求，宿主在结束可变借用后执行原生操作。
#[derive(Clone, Debug, PartialEq)]
pub enum ToolEffect {
    Copy { text: String, sensitive: bool },
    Toast(String),
    Close,
    StartDrag,
}

/// 每个工具拥有自己的实例，默认不暴露可变内部状态。
pub trait ToolModule {
    fn activate(&mut self, input: Option<Selection>) -> Vec<ToolEffect>;
    fn render(&mut self, ui: &mut Ui, palette: &Palette) -> Vec<ToolEffect>;
    fn pinned(&self) -> bool;
    /// 只返回本工具认识的配置字段，由运行时合并到原始配置中。
    fn config(&self) -> Value {
        Value::Object(Default::default())
    }
    fn settings_ui(&mut self, _ui: &mut Ui) {}
    /// 隐藏不等于销毁；敏感结果可在这里主动清除。
    fn on_hidden(&mut self) {}
}

/// 只声明元数据和函数，不构造工具状态或启动后台任务。
#[derive(Clone, Copy)]
pub struct ModuleDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub order: u32,
    pub size: [f32; 2],
    pub toast_offset: f32,
    pub matches: fn(&Selection) -> bool,
    pub paint_icon: fn(&Ui, Rect, &Palette),
    pub create: fn(&Value) -> Box<dyn ToolModule>,
    pub validate_config: fn(&Value) -> bool,
    pub default_hotkey: &'static str,
}

/// 命名空间原样保留未知工具和未知字段，单个损坏配置不影响其他工具。
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleSettings {
    pub version: u32,
    pub enabled: bool,
    pub settings: Value,
    pub window_size: Option<[f32; 2]>,
    pub hotkey: Option<String>,
    pub extra: BTreeMap<String, Value>,
    /// 损坏记录只读保留，序列化时恢复原始内容而非覆盖默认值。
    pub raw_invalid: Option<Value>,
}

impl Default for ModuleSettings {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: true,
            settings: Value::Object(Default::default()),
            window_size: None,
            hotkey: None,
            extra: Default::default(),
            raw_invalid: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct SettingsWire {
    version: u32,
    enabled: bool,
    settings: Value,
    window_size: Option<[f32; 2]>,
    hotkey: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}
impl Default for SettingsWire {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: true,
            settings: Value::Object(Default::default()),
            window_size: None,
            hotkey: None,
            extra: Default::default(),
        }
    }
}
impl Serialize for ModuleSettings {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(raw) = &self.raw_invalid {
            return raw.serialize(serializer);
        }
        SettingsWire {
            version: self.version,
            enabled: self.enabled,
            settings: self.settings.clone(),
            window_size: self.window_size,
            hotkey: self.hotkey.clone(),
            extra: self.extra.clone(),
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ModuleSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(deserializer)?;
        Ok(match serde_json::from_value::<SettingsWire>(raw.clone()) {
            Ok(wire) => Self {
                version: wire.version,
                enabled: wire.enabled,
                settings: wire.settings,
                window_size: wire.window_size,
                hotkey: wire.hotkey,
                extra: wire.extra,
                raw_invalid: None,
            },
            Err(_) => Self {
                version: u32::MAX,
                raw_invalid: Some(raw),
                ..Default::default()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_module_and_unknown_fields_survive_round_trip() {
        let original = serde_json::json!({"broken": {"enabled": "invalid", "future": [1,2]}, "good": {"settings": {"unknown": true}, "future": {"x": 1}}});
        let modules: BTreeMap<String, ModuleSettings> =
            serde_json::from_value(original.clone()).unwrap();
        assert!(modules["broken"].raw_invalid.is_some());
        assert_eq!(
            serde_json::to_value(&modules).unwrap()["broken"],
            original["broken"]
        );
        assert_eq!(
            serde_json::to_value(&modules).unwrap()["good"]["future"],
            original["good"]["future"]
        );
        assert_eq!(modules["good"].version, 1);
    }
}
