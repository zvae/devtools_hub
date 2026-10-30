//! 多个页面共用的小控件：图标按钮、带文字的按钮、固定高度的行、键帽。
//!
//! 与窗口拖动等依赖宿主平台层的控件分开：这里只放平台无关、只依赖配色与图标的部分。

use egui::{Align, FontId, Layout, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::icons::{self, Icon};
use crate::style::{self, Palette};

/// 无边框图标按钮：悬停时显示浅色底，`active` 时用强调色。
pub fn icon_button(ui: &mut Ui, icon: Icon, active: bool, palette: &Palette) -> Response {
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

/// macOS 窗口左上角的关闭按钮：一个红点（与系统窗口的红绿灯一致），悬停时点里出现 ×。
///
/// 只在 macOS 上使用：其他平台的窗口把关闭按钮放在右上角。
#[cfg(target_os = "macos")]
pub fn traffic_light_close(ui: &mut Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    let center = rect.center();
    let painter = ui.painter();
    painter.circle_filled(center, 6.0, egui::Color32::from_rgb(0xff, 0x5f, 0x57));
    if response.hovered() {
        let stroke = Stroke::new(1.2, egui::Color32::from_rgb(0x8c, 0x1c, 0x16));
        let r = 2.7;
        painter.line_segment(
            [center + Vec2::new(-r, -r), center + Vec2::new(r, r)],
            stroke,
        );
        painter.line_segment(
            [center + Vec2::new(r, -r), center + Vec2::new(-r, r)],
            stroke,
        );
    }
    response
}

/// 图标加文字的按钮（底栏的“复制全部”“保存”），悬停时显示浅色底；`enabled` 为 false 时变淡、不响应点击。
pub fn labeled_button(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    palette: &Palette,
) -> Response {
    const HEIGHT: f32 = 26.0;
    const ICON: f32 = 15.0;
    const GAP: f32 = 5.0;
    const PAD: f32 = 10.0;
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), FontId::proportional(12.5), palette.text);
    let width = PAD * 2.0 + ICON + GAP + galley.size().x;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), sense);
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, style::CAPSULE, palette.field);
    }
    let icon_rect = Rect::from_center_size(
        egui::pos2(rect.left() + PAD + ICON / 2.0, rect.center().y),
        Vec2::splat(ICON),
    );
    icons::paint(
        ui.painter(),
        icon_rect,
        icon,
        if enabled {
            palette.weak
        } else {
            palette.weak.gamma_multiply(0.5)
        },
    );
    let text_pos = egui::pos2(
        icon_rect.right() + GAP,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(
        text_pos,
        galley,
        if enabled { palette.text } else { palette.weak },
    );
    response
}

/// 固定高度、内容垂直居中的一行。
///
/// `ui.horizontal` 的初始高度是 `interact_size.y`，先放进去的控件按这个高度居中，
/// 后放进去的更高控件会把行撑高，两者的中线就错开了；固定行高可以避免这种错位。
pub fn centered_row<R>(
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
pub fn keycaps(ui: &mut Ui, keys: &[&str], palette: &Palette) -> Response {
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
