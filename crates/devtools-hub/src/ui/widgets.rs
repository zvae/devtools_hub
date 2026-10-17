//! 多个页面共用的小控件：图标按钮、固定高度的行、键帽，以及栏的拖动区域。

use egui::{Align, FontId, Layout, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::icons::{self, Icon};
use super::style::{self, Palette};
use crate::platform::panel as native_panel;

/// 栏的空白处可以拖动窗口。先于栏里的控件注册，后添加的控件会覆盖它。
pub(super) fn drag_window(ui: &mut Ui, rect: Rect) {
    let drag = ui.interact(rect, ui.id().with("drag-window"), Sense::drag());
    // macOS 由平台层自己发起拖动（winit 的实现可能用错起点，见 `panel::start_drag`）。
    if drag.drag_started() && !native_panel::start_drag() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

/// 无边框图标按钮：悬停时显示浅色底，`active` 时用强调色。
pub(super) fn icon_button(ui: &mut Ui, icon: Icon, active: bool, palette: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    if response.hovered() || active {
        let fill = if active {
            palette.accent.gamma_multiply(0.16)
        } else {
            palette.field
        };
        ui.painter().rect_filled(rect, style::CAPSULE, fill);
    }
    let color = if active { palette.accent } else { palette.weak };
    icons::paint_centered(ui.painter(), rect, 15.0, icon, color);
    response
}

/// 固定高度、内容垂直居中的一行。
///
/// `ui.horizontal` 的初始高度是 `interact_size.y`，先放进去的控件按这个高度居中，
/// 后放进去的更高控件会把行撑高，两者的中线就错开了；固定行高可以避免这种错位。
pub(super) fn centered_row<R>(
    ui: &mut Ui,
    height: f32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_height(height);
            add_contents(ui)
        },
    )
    .inner
}

/// 键帽：把 `["⌘", "V"]` 显示成一个个圆角小方块。
///
/// 先算出总宽度再一次性分配，放在从右到左的布局里也能正确对齐。
/// ↩ 与 ⇥ 在 SF 中没有字形，改为矢量图标绘制。
pub(super) fn keycaps(ui: &mut Ui, keys: &[&str], palette: &Palette) -> Response {
    const HEIGHT: f32 = 20.0;
    const GAP: f32 = 3.0;
    let symbol_icon = |key: &str| match key {
        "↩" => Some(Icon::Return),
        "⇥" => Some(Icon::Tab),
        _ => None,
    };
    let galleys: Vec<_> = keys
        .iter()
        .map(|key| {
            (symbol_icon(key).is_none()).then(|| {
                ui.painter().layout_no_wrap(
                    (*key).to_string(),
                    FontId::proportional(11.5),
                    palette.text,
                )
            })
        })
        .collect();
    let widths: Vec<f32> = galleys
        .iter()
        .map(|galley| {
            galley
                .as_ref()
                .map_or(HEIGHT, |g| (g.size().x + 10.0).max(HEIGHT))
        })
        .collect();
    let total = widths.iter().sum::<f32>() + GAP * widths.len().saturating_sub(1) as f32;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(total, HEIGHT), Sense::hover());
    let mut x = rect.left();
    for ((key, galley), width) in keys.iter().zip(galleys).zip(widths) {
        let cap = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(width, HEIGHT));
        ui.painter().rect(
            cap,
            style::SMALL_RADIUS,
            palette.card,
            Stroke::new(1.0, palette.border),
            StrokeKind::Inside,
        );
        match (galley, symbol_icon(key)) {
            (Some(galley), _) => {
                ui.painter()
                    .galley(cap.center() - galley.size() / 2.0, galley, palette.text);
            }
            (None, Some(icon)) => {
                icons::paint_centered(ui.painter(), cap, 12.0, icon, palette.text)
            }
            (None, None) => {}
        }
        x += width + GAP;
    }
    response
}
