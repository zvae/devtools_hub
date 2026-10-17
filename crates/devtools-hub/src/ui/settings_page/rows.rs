//! 设置页的行控件：分组卡片、左标签右控件的一行、开关、说明文字与分隔线。

use egui::{Align, Color32, Layout, Response, RichText, Sense, Stroke, Ui, Vec2};

use crate::ui::fonts;
use crate::ui::style::{self, Palette};
use crate::ui::widgets::centered_row;

/// 分组标题 + 圆角卡片，返回整个分组的响应（用于滚动到这个分组）。
pub(super) fn section(
    ui: &mut Ui,
    palette: &Palette,
    title: &str,
    add_contents: impl FnOnce(&mut Ui),
) -> Response {
    ui.add_space(12.0);
    // 标题与卡片内的文字左对齐（卡片内边距 12），和系统设置的分组样式一致。
    let heading = egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 12,
            ..Default::default()
        })
        .show(ui, |ui| {
            // 分组标题用小号中粗：与行标题（13、常规、主文字色）拉开层次，
            // 和系统设置里分组标题的写法一致。
            ui.label(
                RichText::new(title)
                    .size(style::TEXT_SECTION)
                    .family(egui::FontFamily::Name(fonts::SEMIBOLD.into()))
                    .color(palette.weak),
            );
        })
        .response;
    ui.add_space(2.0);
    let card = style::continuous_frame(
        ui,
        style::CARD_RADIUS,
        palette.card,
        Stroke::new(1.0, palette.card_stroke),
        egui::Margin::symmetric(12, 8),
        |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            add_contents(ui);
        },
    )
    .response;
    heading.union(card)
}

/// 左侧标签、右侧控件的一行。
pub(super) fn row(ui: &mut Ui, label: &str, add_control: impl FnOnce(&mut Ui)) {
    centered_row(ui, 30.0, |ui| {
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), add_control);
    });
}

/// 水平居中的一行。egui 的水平布局不能把一组控件整体居中：按上一帧量出的内容宽度在左边留白，
/// 宽度变了就丢弃这一帧重新布局，不会先偏一下再居中。
pub(super) fn centered_line(
    ui: &mut Ui,
    id_salt: &str,
    height: f32,
    add_contents: impl FnOnce(&mut Ui),
) {
    let id = ui.id().with(id_salt);
    let available = ui.available_width();
    let width = ui.ctx().data(|data| data.get_temp::<f32>(id));
    centered_row(ui, height, |ui| {
        ui.add_space(((available - width.unwrap_or(available)) / 2.0).max(0.0));
        let start = ui.cursor().left();
        add_contents(ui);
        let measured = ui.min_rect().right() - start;
        if width.is_none_or(|width| (width - measured).abs() > 0.5) {
            ui.ctx().data_mut(|data| data.insert_temp(id, measured));
            ui.ctx().request_discard("居中的一行宽度变了");
        }
    });
}

/// 带开关的一行，返回值是否被切换。
pub(super) fn switch_row(ui: &mut Ui, palette: &Palette, label: &str, value: &mut bool) -> bool {
    let mut changed = false;
    row(ui, label, |ui| {
        changed = toggle(ui, value, palette).changed()
    });
    changed
}

/// 说明文字：比正文小一号，行距放宽（中文换行时更舒展）。
pub(super) fn hint(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(hint_text(text, palette.weak));
}

/// 说明文字的样式，`color` 用于警告等需要换颜色的说明。
pub(super) fn hint_text(text: impl Into<String>, color: Color32) -> RichText {
    RichText::new(text)
        .size(style::TEXT_HINT)
        .line_height(Some(style::HINT_LINE_HEIGHT))
        .color(color)
}

/// 行与行之间的细分隔线。
pub(super) fn divider(ui: &mut Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, palette.card_stroke),
    );
}

/// iOS / macOS 风格的开关。
pub(super) fn toggle(ui: &mut Ui, on: &mut bool, palette: &Palette) -> Response {
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
