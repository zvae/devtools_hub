//! 分类标签、列表与数据分组。

use devtools_clipboard::FAVORITES_LIST_ID;
use egui::{Align, Color32, Layout, RichText, Sense, Ui, Vec2};

use super::rows::{divider, hint, hint_text, row, toggle};
use crate::platform::workspace;
use crate::settings::{Settings, Tab};
use crate::ui::icons::{self, Icon};
use crate::ui::panel::tab_label;
use crate::ui::style::{self, Palette};
use crate::ui::widgets::{centered_row, icon_button};
use crate::ui::HubApp;

impl HubApp {
    /// 面板顶部分类标签的显示与顺序。
    pub(super) fn tabs_section(&mut self, ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
        hint(
            ui,
            palette,
            "“全部”固定在最前面；新建的列表会显示在最后。在面板上右键标签也可以隐藏",
        );
        let lists = &self.settings_view.lists;
        let list_ids: Vec<i64> = lists.iter().map(|list| list.id).collect();
        let mut tabs = settings.ordered_tabs(&list_ids);
        let count = tabs.len();
        let mut changed = false;
        let mut swap = None;
        for (index, setting) in tabs.iter_mut().enumerate() {
            let Some(label) = tab_label(setting.tab, lists) else {
                continue;
            };
            divider(ui, palette);
            let label = label.trim_start_matches("★ ").to_string();
            centered_row(ui, 30.0, |ui| {
                let icon = match setting.tab {
                    Tab::Text => Icon::Document,
                    Tab::Link => Icon::Link,
                    Tab::Image => Icon::Image,
                    Tab::Files => Icon::Folder,
                    Tab::List(FAVORITES_LIST_ID) => Icon::StarFilled,
                    Tab::List(_) => Icon::List,
                };
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                icons::paint(ui.painter(), icon_rect, icon, palette.weak);
                let color = if setting.visible {
                    palette.text
                } else {
                    palette.weak
                };
                ui.label(RichText::new(label).color(color));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    changed |= toggle(ui, &mut setting.visible, palette).changed();
                    ui.add_space(4.0);
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let down = ui
                        .add_enabled_ui(index + 1 < count, |ui| {
                            icon_button(ui, Icon::ChevronDown, false, palette)
                        })
                        .inner;
                    if down.on_hover_text("下移").clicked() {
                        swap = Some((index, index + 1));
                    }
                    let up = ui
                        .add_enabled_ui(index > 0, |ui| {
                            icon_button(ui, Icon::ChevronUp, false, palette)
                        })
                        .inner;
                    if up.on_hover_text("上移").clicked() {
                        swap = Some((index - 1, index));
                    }
                });
            });
        }
        if let Some((a, b)) = swap {
            tabs.swap(a, b);
            changed = true;
        }
        if changed {
            settings.tabs = tabs;
        }
    }

    /// 列表：重命名、删除（内置的“收藏”不能删）与新建。
    pub(super) fn lists_section(&mut self, ui: &mut Ui, palette: &Palette) {
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
                    ui.add(style::text_field(name).desired_width(ui.available_width() - 60.0));
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
                    ui.label(hint_text(format!("{} 条", list.item_count), palette.weak));
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
                    style::text_field(&mut self.settings_view.new_list_name)
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

    /// 数据：记录条数、占用空间、打开数据目录与清空历史（需要再确认一次）。
    pub(super) fn data_section(&mut self, ui: &mut Ui, palette: &Palette) {
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
            ui.label(hint_text(message.clone(), palette.weak));
        }
    }
}
