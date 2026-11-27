//! 普通工具的唯一宿主适配：导航、绘制后请求、配置与动态热键。

use super::{HubApp, Page};
use crate::platform;
use devtools_clipboard::{uti, windows_formats, Representation, Snapshot};
use devtools_module_api::{Selection, ToolEffect};

impl HubApp {
    pub(super) fn open_tool(&mut self, id: &'static str, input: Option<Selection>) {
        let settings = self.settings.modules.get(id).cloned().unwrap_or_default();
        match self.tools.activate(id, input, &settings) {
            Ok(effects) => {
                self.remember_tool_size();
                self.set_page(Page::Tool(id));
                let default = self.tools.descriptor(id).unwrap().size;
                let size = settings
                    .window_size
                    .filter(|s| s.iter().all(|x| x.is_finite() && *x >= 120.0))
                    .unwrap_or(default);
                platform::panel::show(
                    platform::Anchor::Placement(platform::Placement::ScreenCenter),
                    size,
                );
                self.apply_tool_effects(effects);
            }
            Err(error) => self.show_toast(error),
        }
    }

    pub(super) fn tool_ui(&mut self, id: &'static str, ui: &mut egui::Ui) {
        let palette = self.palette(ui.ctx());
        let effects = self.tools.render(id, ui, &palette);
        let settings = self.settings.modules.entry(id.into()).or_default();
        if self.tools.save_config(id, settings) {
            self.persist_settings();
        }
        self.apply_tool_effects(effects);
    }

    fn apply_tool_effects(&mut self, effects: Vec<ToolEffect>) {
        for effect in effects {
            match effect {
                ToolEffect::Toast(text) => self.show_toast(text),
                ToolEffect::Close => self.pending_hide = true,
                ToolEffect::StartDrag => {
                    super::widgets::start_window_drag(&self.ctx);
                }
                ToolEffect::Copy { text, sensitive } => {
                    let snapshot = if sensitive {
                        sensitive_snapshot(&text)
                    } else {
                        Snapshot::from_text(&text)
                    };
                    let pasteboard = self.pasteboard();
                    self.shared.write_pasteboard(|| pasteboard.write(&snapshot));
                }
            }
        }
    }

    pub(super) fn remember_tool_size(&mut self) {
        let Page::Tool(id) = self.page else {
            return;
        };
        let Some(rect) = self.ctx.input(|i| i.viewport().inner_rect) else {
            return;
        };
        let size = [rect.width().round(), rect.height().round()];
        let settings = self.settings.modules.entry(id.into()).or_default();
        if settings.version == 1
            && self.tools.writable(id)
            && size.iter().all(|x| x.is_finite() && *x > 100.0)
            && settings.window_size != Some(size)
        {
            settings.window_size = Some(size);
            self.persist_settings();
        }
    }

    pub(super) fn sync_tool_hotkeys(&mut self) {
        for d in devtools_tools::registrations() {
            let config = self.settings.modules.get(d.id).cloned().unwrap_or_default();
            if !config.enabled {
                self.tools.discard(d.id);
                if self.page == Page::Tool(d.id) {
                    self.set_page(Page::Panel);
                }
            }
            let spec = if config.enabled {
                config.hotkey.as_deref().unwrap_or(d.default_hotkey)
            } else {
                ""
            };
            if let Some(hotkeys) = &mut self.hotkeys {
                match hotkeys.apply_tool(d.id, spec) {
                    Ok(()) => {
                        self.tool_hotkey_errors.remove(d.id);
                    }
                    Err(error) => {
                        self.tool_hotkey_errors.insert(d.id, error);
                    }
                }
            }
        }
    }

    pub(super) fn tool_settings_section(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut crate::settings::Settings,
    ) {
        for d in devtools_tools::registrations() {
            let config = draft.modules.entry(d.id.into()).or_default();
            ui.push_id(d.id, |ui| {
                ui.collapsing(d.name, |ui| {
                    if config.version > 1 {
                        ui.label("工具配置来自较新版本或已损坏，原始值只读保留");
                        return;
                    }
                    ui.checkbox(&mut config.enabled, "启用工具");
                    if config.enabled {
                        let hotkey = config.hotkey.get_or_insert_with(|| d.default_hotkey.into());
                        ui.horizontal(|ui| {
                            ui.label("全局快捷键");
                            ui.text_edit_singleline(hotkey);
                        });
                        ui.label("留空禁用；例如 Cmd+Shift+J 或 Ctrl+Shift+J");
                        if let Some(error) = self.tool_hotkey_errors.get(d.id) {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                        // 仅展开该工具时才创建设置实例，其他工具保持惰性。
                        if let Err(error) = self.tools.settings_ui(d.id, ui, config) {
                            ui.label(error);
                        }
                        self.tools.save_config(d.id, config);
                    }
                });
            });
        }
    }
}

/// 敏感复制是通用能力，工具无需依赖各平台的剪贴板格式。
fn sensitive_snapshot(value: &str) -> Snapshot {
    let mut snapshot = Snapshot::from_text(value);
    snapshot.items[0].push(Representation::new(uti::CONCEALED, Vec::new()));
    for (name, flag) in [
        (windows_formats::EXCLUDE_FROM_MONITOR, 1u32),
        (windows_formats::CAN_INCLUDE_IN_HISTORY, 0),
        ("CanUploadToCloudClipboard", 0),
    ] {
        snapshot.items[0].push(Representation::new(
            format!("{}{name}", windows_formats::RAW_PREFIX),
            flag.to_le_bytes().to_vec(),
        ));
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sensitive_copy_keeps_platform_privacy_markers() {
        let snapshot = sensitive_snapshot("sample-password");
        assert!(snapshot.has_type(uti::CONCEALED));
        let types = snapshot
            .representations()
            .map(|r| r.uti.clone())
            .collect::<Vec<_>>();
        assert!(devtools_clipboard::CaptureRules::default()
            .check_markers(&types, &Default::default())
            .is_err());
        let formats = windows_formats::windows_formats_from_snapshot(&snapshot);
        for (name, flag) in [
            (windows_formats::EXCLUDE_FROM_MONITOR, 1u32),
            (windows_formats::CAN_INCLUDE_IN_HISTORY, 0),
            ("CanUploadToCloudClipboard", 0),
        ] {
            assert!(formats.iter().any(|f| f.target
                == windows_formats::FormatTarget::Registered(name.into())
                && f.data == flag.to_le_bytes()));
        }
    }
}
