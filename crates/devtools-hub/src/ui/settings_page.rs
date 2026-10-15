//! 设置页（对应 PasteNow 偏好设置中的 General / Shortcuts / Exclusion Rules / Advanced）。
//!
//! 采用 macOS 偏好设置常见的“左侧标签、右侧控件”行布局，布尔项使用开关。

use devtools_clipboard::{ClipList, ExcludedApp, SourceApp, Store, StoreStats, FAVORITES_LIST_ID};
use egui::{Align, Color32, Key, Layout, Response, RichText, Sense, Stroke, Ui, Vec2};

use super::fonts;
use super::icons::{self, Icon};
use super::panel::{centered_row, icon_button, keycaps};
use super::style::{self, Palette};
use super::{HubApp, Page};
use crate::hotkey;
use crate::platform::{keyboard, login_item, panel::Placement, workspace};
use crate::settings::{Retention, Settings, POLL_INTERVALS};

#[derive(Default)]
pub struct SettingsView {
    recording_hotkey: bool,
    manual_bundle_id: String,
    new_list_name: String,
    renaming: Option<(i64, String)>,
    confirm_clear: bool,
    stats: StoreStats,
    recent_sources: Vec<SourceApp>,
    lists: Vec<ClipList>,
    launch_at_login: bool,
    message: Option<String>,
}

impl SettingsView {
    pub fn refresh(&mut self, store: &Store) {
        self.stats = store.stats().unwrap_or_default();
        self.recent_sources = store.recent_sources(30).unwrap_or_default();
        self.lists = store.lists().unwrap_or_default();
        self.launch_at_login = login_item::is_enabled();
        self.confirm_clear = false;
        self.message = None;
    }
}

impl HubApp {
    pub(super) fn settings_ui(&mut self, ui: &mut Ui) {
        let palette = self.palette(ui.ctx());
        let escape = !self.settings_view.recording_hotkey
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
        centered_row(ui, 30.0, |ui| {
            if icon_button(ui, Icon::Back, false, &palette).clicked() || escape {
                self.cancel_hotkey_recording();
                self.page = Page::Panel;
                self.panel.reset_for_show();
            }
            ui.label(
                RichText::new("设置")
                    .size(15.0)
                    .family(egui::FontFamily::Name(fonts::SEMIBOLD.into()))
                    .color(palette.text),
            );
        });

        let mut settings = self.settings.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                section(ui, &palette, "通用", |ui| {
                    self.general_section(ui, &palette, &mut settings)
                });
                section(ui, &palette, "粘贴", |ui| {
                    self.paste_section(ui, &palette, &mut settings)
                });
                section(ui, &palette, "记录", |ui| {
                    record_section(ui, &palette, &mut settings)
                });
                section(ui, &palette, "隐私与排除规则", |ui| {
                    self.rules_section(ui, &palette, &mut settings)
                });
                section(ui, &palette, "列表", |ui| {
                    self.lists_section(ui, &palette)
                });
                section(ui, &palette, "数据", |ui| self.data_section(ui, &palette));
                ui.add_space(10.0);
                hint(
                    ui,
                    &palette,
                    &format!(
                        "DevTools Hub {} · 实现思路参考 PasteNow",
                        env!("CARGO_PKG_VERSION")
                    ),
                );
            });

        if settings != self.settings {
            let hotkey_changed = settings.hotkey != self.settings.hotkey;
            self.settings = settings;
            if hotkey_changed {
                self.apply_hotkey();
            }
            self.persist_settings();
        }
    }

    /// 离开设置页时若仍在录制，恢复原快捷键的注册。
    pub(super) fn cancel_hotkey_recording(&mut self) {
        if std::mem::take(&mut self.settings_view.recording_hotkey) {
            self.apply_hotkey();
        }
    }

    fn apply_hotkey(&mut self) {
        self.hotkey_error = match &mut self.hotkeys {
            Some(service) => service.apply(&self.settings.hotkey).err(),
            None => Some("全局快捷键服务不可用".into()),
        };
    }

    fn general_section(&mut self, ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
        if self.settings_view.recording_hotkey {
            row(ui, "唤起快捷键", |ui| {
                ui.label(RichText::new("请按下新的组合键…").color(palette.accent));
            });
            hint(ui, palette, "需要包含 ⌘ ⌥ ⌃ ⇧ 中的至少一个，esc 取消");
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
            if settings.hotkey != default
                && ui.link(RichText::new("恢复默认 ⇧⌘V").size(11.5)).clicked()
            {
                settings.hotkey = default;
            }
        }
        if let Some(error) = &self.hotkey_error {
            ui.label(RichText::new(error).color(palette.warn).size(11.5));
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
            "快捷键唤起时的位置；点击菜单栏图标总是在图标下方弹出",
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
    }

    fn paste_section(&mut self, ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
        switch_row(
            ui,
            palette,
            "直接粘贴到当前应用",
            &mut settings.paste_to_active_app,
        );
        hint(
            ui,
            palette,
            "选中条目后切回之前的应用并模拟 ⌘V；关闭则只复制",
        );
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
        divider(ui, palette);
        switch_row(
            ui,
            palette,
            "粘贴后移到最前",
            &mut settings.move_to_top_after_paste,
        );
    }

    fn rules_section(&mut self, ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
        let rules = &mut settings.rules;
        switch_row(ui, palette, "忽略密码类内容", &mut rules.ignore_concealed);
        hint(ui, palette, "密码管理器标记为 Concealed 的内容");
        divider(ui, palette);
        switch_row(ui, palette, "忽略临时内容", &mut rules.ignore_transient);
        hint(ui, palette, "标记为 Transient 的内容，写入后很快会被还原");
        divider(ui, palette);
        switch_row(
            ui,
            palette,
            "忽略自动生成的内容",
            &mut rules.ignore_auto_generated,
        );
        divider(ui, palette);
        switch_row(
            ui,
            palette,
            "忽略 iPhone / iPad 内容",
            &mut rules.ignore_remote,
        );
        divider(ui, palette);
        switch_row(ui, palette, "忽略空白文本", &mut rules.ignore_blank_text);
        divider(ui, palette);
        row(ui, "大小上限", |ui| {
            ui.add(
                egui::DragValue::new(&mut rules.max_size_mb)
                    .range(0..=2048)
                    .suffix(" MB"),
            );
        });
        hint(ui, palette, "超过该大小的内容不记录，0 表示不限制");
        divider(ui, palette);

        ui.label(RichText::new("不记录以下应用中的复制").color(palette.text));
        ui.add_space(2.0);
        let mut remove = None;
        for (index, app) in rules.excluded_apps.iter().enumerate() {
            centered_row(ui, 28.0, |ui| {
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                if let Some(texture) = self.app_icons.get(ui.ctx(), &app.bundle_id) {
                    egui::Image::from_texture(&texture).paint_at(ui, icon_rect);
                }
                let name = if app.name.is_empty() {
                    &app.bundle_id
                } else {
                    &app.name
                };
                ui.label(RichText::new(name).color(palette.text))
                    .on_hover_text(&app.bundle_id);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, Icon::Close, false, palette)
                        .on_hover_text("移除")
                        .clicked()
                    {
                        remove = Some(index);
                    }
                });
            });
        }
        if let Some(index) = remove {
            rules.excluded_apps.remove(index);
        }
        let candidates: Vec<&SourceApp> =
            self.settings_view
                .recent_sources
                .iter()
                .filter(|source| {
                    source.bundle_id.as_deref().is_some_and(|id| {
                        !rules.excluded_apps.iter().any(|app| app.bundle_id == id)
                    })
                })
                .collect();
        ui.add_space(4.0);
        egui::ComboBox::from_id_salt("exclude-recent")
            .selected_text("从最近的来源应用中添加…")
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for source in candidates {
                    if ui.selectable_label(false, source.display_name()).clicked() {
                        rules.excluded_apps.push(ExcludedApp {
                            bundle_id: source.bundle_id.clone().unwrap_or_default(),
                            name: source.name.clone().unwrap_or_default(),
                        });
                    }
                }
            });
        centered_row(ui, 30.0, |ui| {
            let id = self.settings_view.manual_bundle_id.trim().to_string();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let add = ui.add_enabled(!id.is_empty(), egui::Button::new("添加"));
                if add.clicked() && !rules.excluded_apps.iter().any(|app| app.bundle_id == id) {
                    rules.excluded_apps.push(ExcludedApp {
                        bundle_id: id,
                        name: String::new(),
                    });
                    self.settings_view.manual_bundle_id.clear();
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings_view.manual_bundle_id)
                        .hint_text("或输入 Bundle ID")
                        .desired_width(ui.available_width()),
                );
            });
        });
    }

    fn lists_section(&mut self, ui: &mut Ui, palette: &Palette) {
        hint(
            ui,
            palette,
            "列表中的条目不会被自动清理，也不会被“清空历史”删除",
        );
        let mut changed = false;
        for list in self.settings_view.lists.clone() {
            divider(ui, palette);
            centered_row(ui, 28.0, |ui| match &mut self.settings_view.renaming {
                Some((id, name)) if *id == list.id => {
                    ui.add(
                        egui::TextEdit::singleline(name).desired_width(ui.available_width() - 60.0),
                    );
                    if ui.button("保存").clicked() {
                        if let Err(err) = self.store.rename_list(list.id, name) {
                            self.settings_view.message = Some(format!("{err:#}"));
                        }
                        self.settings_view.renaming = None;
                        changed = true;
                    }
                }
                _ => {
                    if list.id == FAVORITES_LIST_ID {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                        icons::paint(ui.painter(), icon_rect, Icon::StarFilled, palette.weak);
                    }
                    ui.label(RichText::new(&list.name).color(palette.text));
                    ui.label(
                        RichText::new(format!("{} 条", list.item_count))
                            .color(palette.weak)
                            .size(11.5),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if list.id != FAVORITES_LIST_ID
                            && icon_button(ui, Icon::Close, false, palette)
                                .on_hover_text("删除列表（条目保留在历史中）")
                                .clicked()
                        {
                            if let Err(err) = self.store.delete_list(list.id) {
                                self.settings_view.message = Some(format!("{err:#}"));
                            }
                            changed = true;
                        }
                        if icon_button(ui, Icon::Pencil, false, palette)
                            .on_hover_text("重命名")
                            .clicked()
                        {
                            self.settings_view.renaming = Some((list.id, list.name.clone()));
                        }
                    });
                }
            });
        }
        divider(ui, palette);
        centered_row(ui, 30.0, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("新建").clicked() {
                    match self.store.create_list(&self.settings_view.new_list_name) {
                        Ok(_) => self.settings_view.new_list_name.clear(),
                        Err(err) => self.settings_view.message = Some(format!("{err:#}")),
                    }
                    changed = true;
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings_view.new_list_name)
                        .hint_text("新列表名称")
                        .desired_width(ui.available_width()),
                );
            });
        });
        if changed {
            self.settings_view.lists = self.store.lists().unwrap_or_default();
            self.panel.needs_reload = true;
        }
    }

    fn data_section(&mut self, ui: &mut Ui, palette: &Palette) {
        let stats = self.settings_view.stats;
        row(ui, "记录条数", |ui| {
            ui.label(RichText::new(style::group_digits(stats.item_count)).color(palette.text));
        });
        divider(ui, palette);
        row(ui, "占用空间", |ui| {
            let total = stats.database_bytes + stats.external_bytes;
            ui.label(RichText::new(style::byte_size(total)).color(palette.text));
        });
        hint(ui, palette, &self.data_dir.display().to_string());
        divider(ui, palette);
        centered_row(ui, 30.0, |ui| {
            if ui.button("打开数据目录").clicked() {
                workspace::open_path(&self.data_dir);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.settings_view.confirm_clear {
                    if ui.button("取消").clicked() {
                        self.settings_view.confirm_clear = false;
                    }
                    let confirm =
                        egui::Button::new(RichText::new("确定清空").color(Color32::WHITE))
                            .fill(Color32::from_rgb(0xe5, 0x48, 0x4d));
                    if ui.add(confirm).clicked() {
                        match self.store.clear_history() {
                            Ok(removed) => {
                                self.settings_view.message =
                                    Some(format!("已删除 {removed} 条记录"))
                            }
                            Err(err) => self.settings_view.message = Some(format!("{err:#}")),
                        }
                        self.settings_view.confirm_clear = false;
                        self.settings_view.stats = self.store.stats().unwrap_or_default();
                        self.panel.needs_reload = true;
                    }
                } else if ui.button("清空历史…").clicked() {
                    self.settings_view.confirm_clear = true;
                }
            });
        });
        if self.settings_view.confirm_clear {
            hint(ui, palette, "清空后无法恢复；列表中的条目会保留");
        }
        if let Some(message) = &self.settings_view.message {
            ui.label(RichText::new(message).color(palette.weak).size(11.5));
        }
    }
}

fn record_section(ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
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

/// 分组标题 + 圆角卡片。
fn section(ui: &mut Ui, palette: &Palette, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    ui.add_space(12.0);
    ui.label(RichText::new(title).size(12.0).color(palette.weak));
    ui.add_space(2.0);
    egui::Frame::new()
        .fill(palette.card)
        .stroke(Stroke::new(1.0, palette.card_stroke))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            add_contents(ui);
        });
}

/// 左侧标签、右侧控件的一行。
fn row(ui: &mut Ui, label: &str, add_control: impl FnOnce(&mut Ui)) {
    centered_row(ui, 30.0, |ui| {
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), add_control);
    });
}

/// 带开关的一行，返回值是否被切换。
fn switch_row(ui: &mut Ui, palette: &Palette, label: &str, value: &mut bool) -> bool {
    let mut changed = false;
    row(ui, label, |ui| {
        changed = toggle(ui, value, palette).changed()
    });
    changed
}

fn hint(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(palette.weak));
}

fn divider(ui: &mut Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, palette.card_stroke),
    );
}

/// iOS / macOS 风格的开关。
fn toggle(ui: &mut Ui, on: &mut bool, palette: &Palette) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(36.0, 20.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let track = palette.track.lerp_to_gamma(palette.accent, t);
    ui.painter().rect_filled(rect, rect.height() / 2.0, track);
    let radius = rect.height() / 2.0 - 2.0;
    let x = egui::lerp(
        (rect.left() + radius + 2.0)..=(rect.right() - radius - 2.0),
        t,
    );
    ui.painter().circle(
        egui::pos2(x, rect.center().y),
        radius,
        Color32::WHITE,
        Stroke::new(0.5, Color32::from_black_alpha(30)),
    );
    response
}
