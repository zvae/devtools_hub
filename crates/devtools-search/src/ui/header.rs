//! 顶栏：搜索卡片（放大镜 + 输入框 + Aa / 历史 / 清除）、范围胶囊与设置入口（§5.1、§5.3）。
//!
//! 视觉与剪贴板面板一致：卡片底色 + 连续圆角、胶囊按钮、矢量图标，不用裸字符按钮。

use egui::{Align, FontId, Layout, Rect, Sense, Stroke, Ui, Vec2};

use super::icons::{self, Icon};
use super::{theme, ViewState};
use crate::application::service::Service;

/// 顶栏产生的动作（由模块转成命令）。
#[derive(Default)]
pub struct HeaderAction {
    pub submit: bool,
    pub open_settings: bool,
    pub toggle_case: bool,
    pub toggle_history: bool,
    pub toggle_saved: bool,
    /// 按住顶栏空白处拖动窗口。
    pub drag: bool,
}

pub fn show(ui: &mut Ui, view: &mut ViewState, service: &Service) -> HeaderAction {
    let palette = theme::palette(ui);
    let mut action = HeaderAction::default();
    let width = ui.available_width();
    let height = theme::HEADER_HEIGHT;
    view.header_height = height;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());

    // 顶栏留白可以拖动窗口（§4.2：宿主平台层负责真正的拖动）。
    let drag_rect = Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + 11.0));
    let drag = ui.interact(
        drag_rect,
        ui.id().with("search-drag"),
        Sense::click_and_drag(),
    );
    action.drag = drag.drag_started();

    let row = Rect::from_min_size(
        egui::pos2(rect.left() + 14.0, rect.top() + 12.0),
        Vec2::new(rect.width() - 28.0, 34.0),
    );
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = 8.0;

    // 范围胶囊（入口带来的目录范围）占右侧一段：先量出它需要的宽度。
    let scope_label = view.scope.as_ref().map(|scope| {
        theme::middle_ellipsis(
            ui,
            &format!("范围：{}", scope.to_string_lossy()),
            FontId::proportional(theme::TEXT_HINT),
            190.0,
        )
    });
    let chip_width = scope_label
        .as_ref()
        .map(|label| {
            let galley = ui.painter().layout_no_wrap(
                label.clone(),
                FontId::proportional(theme::TEXT_HINT),
                palette.text,
            );
            (galley.size().x + 24.0 + 22.0).clamp(96.0, 246.0)
        })
        .unwrap_or(0.0);

    // 搜索卡片：占剩余宽度（右侧留出范围胶囊与设置按钮）。
    let reserved = 26.0 + 8.0 + if chip_width > 0.0 { chip_width + 8.0 } else { 0.0 };
    let card_width = (row.width() - reserved).max(220.0);
    let (card_rect, _) = bar.allocate_exact_size(Vec2::new(card_width, 34.0), Sense::hover());
    let focused = ui.memory(|m| m.has_focus(ui.id().with("search-field")));
    ui.painter().add(theme::continuous_rect(
        card_rect,
        12.0,
        palette.card,
        Stroke::new(
            if focused { 1.5 } else { 1.0 },
            if focused {
                palette.accent.gamma_multiply(0.85)
            } else {
                palette.card_stroke
            },
        ),
    ));
    icons::paint_centered(
        ui.painter(),
        Rect::from_center_size(
            egui::pos2(card_rect.left() + 16.0, card_rect.center().y),
            Vec2::splat(14.0),
        ),
        14.0,
        Icon::Search,
        palette.weak,
    );

    // 卡片右侧控件（清除、历史、Aa）。
    let history = service.settings().history_enabled;
    // 星标按钮常驻：保存 / 复用搜索。
    let trailing_width = match (view.query.is_empty(), history) {
        (true, true) => 92.0,
        (true, false) => 62.0,
        (false, true) => 120.0,
        (false, false) => 90.0,
    };
    // 输入框按"一行文字的实际高度"垂直居中放在卡片里，占位文字自己画在同一基线上，
    // 避免 TextEdit 的 hint 与正文上下错位（中文行高与西文不同）。
    let text_height = ui
        .painter()
        .layout_no_wrap(
            "搜索Ag".to_string(),
            FontId::proportional(theme::TEXT_BODY),
            palette.text,
        )
        .size()
        .y;
    let edit_left = card_rect.left() + 30.0;
    let edit_width = (card_rect.right() - trailing_width - edit_left).max(40.0);
    let edit_rect = Rect::from_min_size(
        egui::pos2(edit_left, card_rect.center().y - text_height / 2.0),
        Vec2::new(edit_width, text_height),
    );
    let response = ui.put(
        edit_rect,
        egui::TextEdit::singleline(&mut view.query)
            .id(ui.id().with("search-field"))
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(edit_width),
    );
    if view.query.is_empty() {
        ui.painter().text(
            egui::pos2(edit_left, card_rect.center().y),
            egui::Align2::LEFT_CENTER,
            "搜索文件，支持 ext:pdf、folder:、project/report",
            FontId::proportional(theme::TEXT_BODY),
            palette.weak,
        );
    }
    if response.changed() {
        view.last_edit = Some(std::time::Instant::now());
        // 手动编辑后不再按入口的字面短语解释。
        view.literal = false;
    }
    if view.focus_pending {
        response.request_focus();
        view.focus_pending = false;
    }
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        action.submit = true;
    }

    let mut trailing = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                egui::pos2(card_rect.right() - trailing_width, card_rect.top() + 4.0),
                egui::pos2(card_rect.right() - 6.0, card_rect.bottom() - 4.0),
            ))
            .layout(Layout::right_to_left(Align::Center)),
    );
    trailing.spacing_mut().item_spacing.x = 2.0;
    if theme::pill_button(&mut trailing, &palette, "Aa", view.case_sensitive)
        .on_hover_text("区分大小写")
        .clicked()
    {
        action.toggle_case = true;
    }
    if theme::icon_button(
        &mut trailing,
        if view.saved_open {
            Icon::StarFilled
        } else {
            Icon::Star
        },
        view.saved_open,
        &palette,
    )
    .on_hover_text("保存的搜索")
    .clicked()
    {
        action.toggle_saved = true;
    }
    if history
        && theme::icon_button(&mut trailing, Icon::Clock, view.history_open, &palette)
            .on_hover_text("搜索历史")
            .clicked()
    {
        action.toggle_history = true;
    }
    if !view.query.is_empty()
        && theme::icon_button(&mut trailing, Icon::Close, false, &palette)
            .on_hover_text("清空")
            .clicked()
    {
        view.query.clear();
        view.last_edit = None;
        action.submit = true;
    }

    // 范围胶囊（点击清除）。
    if let (Some(scope), Some(label)) = (view.scope.clone(), scope_label) {
        let (chip, response) = bar.allocate_exact_size(Vec2::new(chip_width, 22.0), Sense::click());
        let fill = if response.hovered() {
            palette.accent.gamma_multiply(0.14)
        } else {
            palette.field
        };
        ui.painter().rect_filled(chip, 11.0, fill);
        icons::paint_centered(
            ui.painter(),
            Rect::from_center_size(
                egui::pos2(chip.left() + 13.0, chip.center().y),
                Vec2::splat(12.0),
            ),
            12.0,
            Icon::Folder,
            palette.accent,
        );
        let galley =
            ui.painter()
                .layout_no_wrap(label, FontId::proportional(theme::TEXT_HINT), palette.text);
        let clipped = ui.painter().with_clip_rect(Rect::from_min_max(
            egui::pos2(chip.left() + 23.0, chip.top()),
            egui::pos2(chip.right() - 20.0, chip.bottom()),
        ));
        clipped.galley(
            egui::pos2(chip.left() + 23.0, chip.center().y - galley.size().y / 2.0),
            galley,
            palette.text,
        );
        icons::paint_centered(
            ui.painter(),
            Rect::from_center_size(
                egui::pos2(chip.right() - 11.0, chip.center().y),
                Vec2::splat(10.0),
            ),
            10.0,
            Icon::Close,
            palette.weak,
        );
        let tip = format!("范围：{}\n点击清除", scope.display());
        if response.on_hover_text(tip).clicked() {
            view.scope = None;
            action.submit = true;
        }
    }

    // 设置齿轮。
    if theme::icon_button(&mut bar, Icon::Gear, false, &palette)
        .on_hover_text("搜索设置")
        .clicked()
    {
        action.open_settings = true;
    }

    // 底部分隔线。
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, palette.border.gamma_multiply(0.6)),
    );
    action
}
