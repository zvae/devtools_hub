//! egui 弹窗里的菜单项（右上角“+”的新建列表弹窗）。右键菜单用的是原生菜单（见 `native_menu`），
//! 这里的样式与它保持一致：只有文字，悬停时整行填充强调色，分隔线左右内缩。
//! 弹窗底板（底色、描边、阴影）在 `style::install` 里设置。

use egui::{Color32, FontId, Response, Sense, Stroke, Ui, Vec2, WidgetInfo, WidgetType};

use super::style::Palette;

/// 菜单项高度与标准控件一致；高亮是胶囊形，与弹窗外框同心（`style::MENU_RADIUS`）。
const HEIGHT: f32 = 24.0;
/// 文字到高亮边缘的留白。
const PAD: f32 = 10.0;

/// 弹窗内容的公共设置：菜单项紧挨着排列，并限制最小宽度。
pub fn begin(ui: &mut Ui, min_width: f32) {
    ui.spacing_mut().item_spacing.y = 0.0;
    ui.set_min_width(min_width);
}

/// 一个菜单项：整行可点，悬停时显示强调色底。
pub fn item(ui: &mut Ui, palette: &Palette, label: &str) -> Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        FontId::proportional(13.0),
        Color32::PLACEHOLDER,
    );
    let mut width = PAD * 2.0 + galley.size().x;
    if ui.layout().horizontal_justify() {
        width = width.max(ui.available_width());
    }
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    let color = if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, HEIGHT / 2.0, palette.accent);
        Color32::WHITE
    } else {
        palette.text
    };
    let pos = egui::pos2(rect.left() + PAD, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, color);
    response
}

/// 小标题，与菜单项文字左对齐。
pub fn heading(ui: &mut Ui, palette: &Palette, text: &str) {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), FontId::proportional(12.0), palette.weak);
    let size = Vec2::new(PAD * 2.0 + galley.size().x, HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let pos = egui::pos2(rect.left() + PAD, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, palette.weak);
}

/// 左右内缩的细分隔线。
pub fn separator(ui: &mut Ui, palette: &Palette) {
    // 宽度为 0：不撑宽弹窗，线条按弹窗内容区的宽度画。
    let (rect, _) = ui.allocate_exact_size(Vec2::new(0.0, 9.0), Sense::hover());
    let x = ui.max_rect().x_range();
    let y = ui.painter().round_to_pixel_center(rect.center().y);
    ui.painter().hline(
        x.min + PAD..=x.max - PAD,
        y,
        Stroke::new(1.0, palette.border),
    );
}
