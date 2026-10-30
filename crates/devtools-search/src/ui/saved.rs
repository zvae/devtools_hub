//! 保存的搜索（书签）：保存当前查询 / 范围，之后一键复用（§5.4、§6.1）。

use egui::{Align2, FontId, Rect, RichText, Sense, Stroke, Ui, Vec2};

use super::icons::{self, Icon};
use super::{theme, ViewState};
use crate::application::service::Service;
use crate::settings::SavedSearch;

/// 下拉菜单的动作。
#[derive(Default)]
pub struct SavedAction {
    /// 应用某条保存的搜索。
    pub apply: Option<usize>,
    /// 删除某条。
    pub delete: Option<usize>,
    /// 保存当前查询。
    pub save: bool,
}

pub fn show(ui: &mut Ui, view: &mut ViewState, service: &Service) -> SavedAction {
    let palette = theme::palette(ui);
    let mut action = SavedAction::default();
    let entries: Vec<SavedSearch> = service.settings().saved_searches.clone();
    let can_save = !view.query.trim().is_empty() || view.scope.is_some();
    let origin = egui::pos2(14.0, view.header_height + 4.0);

    egui::Area::new(egui::Id::new("search-saved"))
        .order(egui::Order::Foreground)
        .fixed_pos(origin)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(if palette.dark {
                    egui::Color32::from_rgb(44, 44, 48)
                } else {
                    egui::Color32::WHITE
                })
                .corner_radius(f32::from(theme::MENU_RADIUS))
                .stroke(Stroke::new(1.0, palette.border))
                .shadow(egui::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(if palette.dark { 110 } else { 40 }),
                })
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(320.0);
                    if entries.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("还没有保存的搜索")
                                    .size(theme::TEXT_HINT)
                                    .color(palette.faint),
                            );
                        });
                    }
                    for (index, entry) in entries.iter().enumerate() {
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 30.0),
                            Sense::click(),
                        );
                        if response.hovered() {
                            ui.painter().rect_filled(rect, 6.0, palette.field);
                        }
                        icons::paint_centered(
                            ui.painter(),
                            Rect::from_center_size(
                                egui::pos2(rect.left() + 14.0, rect.center().y),
                                Vec2::splat(12.0),
                            ),
                            12.0,
                            Icon::StarFilled,
                            palette.accent,
                        );
                        // 名称 + （范围）说明。
                        let label = if let Some(scope) = &entry.scope {
                            format!(
                                "{}   ·   {}",
                                entry.name,
                                theme::middle_ellipsis(
                                    ui,
                                    &scope.to_string_lossy(),
                                    FontId::proportional(theme::TEXT_HINT),
                                    120.0
                                )
                            )
                        } else {
                            entry.name.clone()
                        };
                        let job = egui::text::LayoutJob::simple_singleline(
                            label,
                            FontId::proportional(12.5),
                            palette.text,
                        );
                        let mut job = job;
                        job.wrap = egui::text::TextWrapping::truncate_at_width(
                            (rect.width() - 70.0).max(80.0),
                        );
                        let galley = ui.painter().layout_job(job);
                        ui.painter().galley(
                            egui::pos2(rect.left() + 26.0, rect.center().y - galley.size().y / 2.0),
                            galley,
                            palette.text,
                        );
                        // 删除按钮。
                        let mut remove_ui = ui.new_child(
                            egui::UiBuilder::new().max_rect(Rect::from_min_size(
                                egui::pos2(rect.right() - 28.0, rect.top()),
                                Vec2::new(28.0, 30.0),
                            )),
                        );
                        if theme::icon_button(&mut remove_ui, Icon::Trash, false, &palette)
                            .on_hover_text("删除这条保存的搜索")
                            .clicked()
                        {
                            action.delete = Some(index);
                        } else if response.clicked() {
                            action.apply = Some(index);
                        }
                    }
                    ui.add_space(2.0);
                    theme::divider(ui, &palette);
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 28.0),
                        if can_save { Sense::click() } else { Sense::hover() },
                    );
                    if can_save && response.hovered() {
                        ui.painter().rect_filled(rect, 6.0, palette.field);
                    }
                    icons::paint_centered(
                        ui.painter(),
                        Rect::from_center_size(
                            egui::pos2(rect.left() + 14.0, rect.center().y),
                            Vec2::splat(12.0),
                        ),
                        12.0,
                        Icon::Plus,
                        if can_save { palette.accent } else { palette.faint },
                    );
                    ui.painter().text(
                        egui::pos2(rect.left() + 26.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        "保存当前搜索",
                        FontId::proportional(12.5),
                        if can_save { palette.text } else { palette.faint },
                    );
                    if can_save && response.clicked() {
                        action.save = true;
                    }
                });
        });
    action
}
