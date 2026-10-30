//! 历史下拉（§5.4）：最近查询，点击即执行；可清空。

use egui::{Align2, FontId, Rect, RichText, Sense, Stroke, Ui, Vec2};

use super::icons::{self, Icon};
use super::{theme, ViewState};
use crate::application::service::Service;

/// 历史下拉的动作。
#[derive(Default)]
pub struct HistoryAction {
    pub pick: Option<String>,
    pub clear: bool,
}

pub fn show(ui: &mut Ui, view: &mut ViewState, service: &Service) -> HistoryAction {
    let palette = theme::palette(ui);
    let mut action = HistoryAction::default();
    let entries: Vec<_> = service
        .settings()
        .history
        .iter()
        .take(10)
        .cloned()
        .collect();
    let default_pos = egui::pos2(14.0, view.header_height + 4.0);

    egui::Area::new(egui::Id::new("search-history"))
        .order(egui::Order::Foreground)
        .fixed_pos(default_pos)
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
                    ui.set_width(300.0);
                    if entries.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("还没有搜索历史")
                                    .size(theme::TEXT_HINT)
                                    .color(palette.faint),
                            );
                        });
                    }
                    for entry in &entries {
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 26.0),
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
                            Icon::Clock,
                            palette.weak,
                        );
                        ui.painter().text(
                            egui::pos2(rect.left() + 28.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            if entry.query.chars().count() > 26 {
                                format!("{}…", entry.query.chars().take(25).collect::<String>())
                            } else {
                                entry.query.clone()
                            },
                            FontId::proportional(12.5),
                            palette.text,
                        );
                        if let Some(scope) = &entry.scope {
                            ui.painter().text(
                                egui::pos2(rect.right() - 8.0, rect.center().y),
                                Align2::RIGHT_CENTER,
                                theme::middle_ellipsis(
                                    ui,
                                    &scope.to_string_lossy(),
                                    FontId::proportional(theme::TEXT_HINT),
                                    120.0,
                                ),
                                FontId::proportional(theme::TEXT_HINT),
                                palette.faint,
                            );
                        }
                        if response.clicked() {
                            action.pick = Some(entry.query.clone());
                        }
                    }
                    if !entries.is_empty() {
                        ui.add_space(2.0);
                        theme::divider(ui, &palette);
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 26.0),
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
                            Icon::Trash,
                            palette.danger,
                        );
                        ui.painter().text(
                            egui::pos2(rect.left() + 28.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            "清空历史",
                            FontId::proportional(12.5),
                            palette.danger,
                        );
                        if response.clicked() {
                            action.clear = true;
                        }
                    }
                });
        });
    action
}
