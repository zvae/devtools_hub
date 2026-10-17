//! 隐私与排除规则分组：忽略哪些类型的内容、大小上限、不记录哪些应用中的复制。

use devtools_clipboard::{ExcludedApp, SourceApp};
use egui::{Align, Layout, RichText, Sense, Ui, Vec2};

use super::rows::{divider, hint, row, switch_row};
use crate::platform;
use crate::settings::Settings;
use crate::ui::icons::Icon;
use crate::ui::style::{self, Palette};
use crate::ui::widgets::{centered_row, icon_button};
use crate::ui::HubApp;

impl HubApp {
    /// 排除规则：各类“忽略”开关、大小上限，以及不记录的应用（从最近的来源应用中选，或手动输入）。
    pub(super) fn rules_section(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        settings: &mut Settings,
    ) {
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
        if platform::HAS_UNIVERSAL_CLIPBOARD {
            switch_row(
                ui,
                palette,
                "忽略 iPhone / iPad 内容",
                &mut rules.ignore_remote,
            );
            divider(ui, palette);
        }
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
                    style::text_field(&mut self.settings_view.manual_bundle_id)
                        .hint_text(if cfg!(windows) {
                            "或输入程序名，如 KeePass.exe"
                        } else {
                            "或输入 Bundle ID"
                        })
                        .desired_width(ui.available_width()),
                );
            });
        });
    }
}
