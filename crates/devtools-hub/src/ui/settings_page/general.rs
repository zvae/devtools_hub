//! 通用、外观、粘贴、鼠标中键与记录分组。

use egui::{Key, RichText, Sense, Ui, Vec2};

use super::rows::{divider, hint, hint_text, row, switch_row};
use crate::platform::{keyboard, login_item, panel::Placement};
use crate::settings::{Retention, Settings, POLL_INTERVALS};
use crate::ui::icons::{self, Icon};
use crate::ui::style::Palette;
use crate::ui::widgets::keycaps;
use crate::ui::HubApp;
use crate::{hotkey, keys, platform};

impl HubApp {
    /// 通用：唤起快捷键（录制）、出现位置、失焦隐藏、登录时启动、自动检查更新。
    pub(super) fn general_section(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        settings: &mut Settings,
    ) {
        if self.settings_view.recording_hotkey {
            row(ui, "唤起快捷键", |ui| {
                ui.label(RichText::new("请按下新的组合键…").color(palette.accent));
            });
            hint(
                ui,
                palette,
                &format!(
                    "需要包含 {} 中的至少一个，{} 取消",
                    keys::MODIFIER_HINT,
                    keys::ESCAPE
                ),
            );
            let pressed = ui.input(|i| {
                i.events.iter().find_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            match pressed {
                Some((Key::Escape, _)) => {
                    self.settings_view.recording_hotkey = false;
                    self.apply_hotkey();
                }
                Some((key, modifiers)) => {
                    if let Some(spec) = hotkey::from_egui(modifiers, key) {
                        settings.hotkey = spec;
                        self.settings_view.recording_hotkey = false;
                        // 录制期间注销了旧快捷键，这里无论是否变化都重新注册。
                        if settings.hotkey == self.settings.hotkey {
                            self.apply_hotkey();
                        }
                    }
                }
                None => {}
            }
        } else {
            row(ui, "唤起快捷键", |ui| {
                if ui.button("录制").clicked() {
                    self.settings_view.recording_hotkey = true;
                    // 避免录制时按下旧快捷键直接把面板关掉。
                    if let Some(service) = &mut self.hotkeys {
                        service.suspend();
                    }
                }
                let parts = hotkey::display_parts(&settings.hotkey);
                let keys: Vec<&str> = parts.iter().map(String::as_str).collect();
                keycaps(ui, &keys, palette);
            });
            let default = Settings::default().hotkey;
            let restore = format!("恢复默认 {}", hotkey::display(&default));
            if settings.hotkey != default && ui.link(RichText::new(restore).size(11.5)).clicked() {
                settings.hotkey = default;
            }
        }
        if let Some(error) = &self.hotkey_error {
            ui.label(hint_text(error.clone(), palette.warn));
        }
        divider(ui, palette);
        row(ui, "出现位置", |ui| {
            egui::ComboBox::from_id_salt("placement")
                .width(120.0)
                .selected_text(settings.placement.label())
                .show_ui(ui, |ui| {
                    for placement in Placement::ALL {
                        ui.selectable_value(&mut settings.placement, placement, placement.label());
                    }
                });
        });
        hint(
            ui,
            palette,
            &format!(
                "快捷键唤起时的位置；点击{}时总是在图标旁弹出",
                platform::TRAY_NAME
            ),
        );
        divider(ui, palette);
        switch_row(ui, palette, "失焦时自动隐藏", &mut settings.hide_on_blur);
        divider(ui, palette);
        let mut launch = self.settings_view.launch_at_login;
        if switch_row(ui, palette, "登录时启动", &mut launch) {
            match login_item::set_enabled(launch) {
                Ok(()) => self.settings_view.launch_at_login = launch,
                Err(err) => self.settings_view.message = Some(format!("{err:#}")),
            }
        }
        divider(ui, palette);
        switch_row(
            ui,
            palette,
            "自动检查更新",
            &mut settings.auto_check_updates,
        );
        hint(
            ui,
            palette,
            "启动时和之后每天检查一次 GitHub 上是否有新版本",
        );
    }

    /// 粘贴：是否直接粘贴到当前应用、辅助功能权限、粘贴后是否移到最前。
    pub(super) fn paste_section(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        settings: &mut Settings,
    ) {
        switch_row(
            ui,
            palette,
            "直接粘贴到当前应用",
            &mut settings.paste_to_active_app,
        );
        hint(
            ui,
            palette,
            &format!(
                "选中条目后切回之前的应用并模拟 {}；关闭则只复制",
                keys::shortcut(&[keys::COMMAND, "V"])
            ),
        );
        // 只有 macOS 需要授权；Windows 模拟按键不需要。
        if platform::PASTE_NEEDS_PERMISSION {
            divider(ui, palette);
            let trusted = keyboard::accessibility_trusted(false);
            self.accessibility_ok = trusted;
            row(ui, "辅助功能权限", |ui| {
                if trusted {
                    ui.label(RichText::new("已授权").color(palette.ok));
                    let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    icons::paint(ui.painter(), icon_rect, Icon::Check, palette.ok);
                } else if ui.button("去授权").clicked() {
                    keyboard::accessibility_trusted(true);
                    keyboard::open_accessibility_settings();
                }
            });
            if !trusted {
                // 系统按签名识别应用：授权记录属于签名不同的旧版本时，开关显示已打开但不生效。
                hint(
                    ui,
                    palette,
                    "系统设置里已经打开却仍显示未授权：在“辅助功能”列表中选中 DevTools Hub，\
                     点“−”删掉，再点“去授权”重新添加",
                );
            }
        }
        divider(ui, palette);
        switch_row(
            ui,
            palette,
            "粘贴后移到最前",
            &mut settings.move_to_top_after_paste,
        );
    }
}

/// 鼠标中键：是否按中键弹出快捷面板。
pub(super) fn middle_click_section(ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
    switch_row(
        ui,
        palette,
        "按下鼠标中键弹出快捷面板",
        &mut settings.middle_click,
    );
    hint(
        ui,
        palette,
        &format!(
            "在其他应用中选中文字后按鼠标中键，会模拟 {} 读取选中的内容（读完恢复剪贴板），\
             列出能处理它的功能，例如选中 JSON 时可以用 JSON 编辑器格式化；没有选中时列出常用功能",
            keys::shortcut(&[keys::COMMAND, "C"])
        ),
    );
    if platform::PASTE_NEEDS_PERMISSION && settings.middle_click {
        if keyboard::accessibility_trusted(false) {
            hint(ui, palette, "开启后，中键点击不会再传给前台应用");
        } else {
            ui.label(hint_text(
                "需要“辅助功能”权限才能监听鼠标中键和读取选中的内容",
                palette.warn,
            ));
        }
    }
}

/// 记录：暂停、检查剪贴板的间隔、保留时长与最多保留条数。
pub(super) fn record_section(ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
    switch_row(ui, palette, "暂停记录", &mut settings.paused);
    divider(ui, palette);
    row(ui, "检查间隔", |ui| {
        let current = POLL_INTERVALS
            .iter()
            .find(|(ms, _)| *ms == settings.poll_interval_ms)
            .map_or_else(
                || format!("{} ms", settings.poll_interval_ms),
                |(_, label)| label.to_string(),
            );
        egui::ComboBox::from_id_salt("poll")
            .width(140.0)
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (ms, label) in POLL_INTERVALS {
                    ui.selectable_value(&mut settings.poll_interval_ms, ms, label);
                }
            });
    });
    divider(ui, palette);
    row(ui, "保留时长", |ui| {
        egui::ComboBox::from_id_salt("retention")
            .width(140.0)
            .selected_text(settings.retention.label())
            .show_ui(ui, |ui| {
                for retention in Retention::ALL {
                    ui.selectable_value(&mut settings.retention, retention, retention.label());
                }
            });
    });
    divider(ui, palette);
    row(ui, "最多保留", |ui| {
        ui.add(
            egui::DragValue::new(&mut settings.max_items)
                .range(0..=1_000_000)
                .suffix(" 条"),
        );
    });
    hint(ui, palette, "0 表示不限制；列表中的条目不计入");
}

/// 外观：面板背景的透明度（只在有原生玻璃背景时显示）。
pub(super) fn appearance_section(ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
    row(ui, "背景透明度", |ui| {
        // 行是从右到左排的：先放数值（固定宽度，拖动时滑块不跟着挪），再放滑块。
        let value = RichText::new(format!("{}%", settings.panel_transparency)).color(palette.weak);
        ui.add_sized([34.0, 20.0], egui::Label::new(value));
        ui.spacing_mut().slider_width = 150.0;
        // 滑块会把圆角加到填充条的长度上，全局的胶囊圆角（255）会让填充条越界，换成轨道高度的一半。
        let rail_radius = (ui.spacing().slider_rail_height / 2.0) as u8;
        ui.visuals_mut().widgets.inactive.corner_radius = rail_radius.into();
        ui.add(
            egui::Slider::new(&mut settings.panel_transparency, 0..=100)
                .show_value(false)
                .trailing_fill(true),
        );
    });
    hint(
        ui,
        palette,
        "面板底色的透明度：越大越能透出背后的玻璃效果，越小文字越清晰",
    );
}
