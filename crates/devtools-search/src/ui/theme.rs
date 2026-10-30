//! 搜索页的视觉规范。
//!
//! 配色、圆角、字号、连续曲率圆角与通用小控件全部来自共用模块 `devtools-ui`
//! （`devtools_ui::style` 与 `devtools_ui::widgets`），这里只保留搜索页自己的
//! 布局度量、依赖这些度量的组合控件，以及时间 / 数字 / 大小的格式化。

use egui::{Align, Color32, FontFamily, FontId, Layout, Response, RichText, Sense, Stroke, Ui, Vec2};

// 基础样式直接复用共用模块，保持 `ui::theme::*` 的调用路径不变。
pub use devtools_ui::style::{
    continuous_frame, continuous_rect, palette, Palette, CAPSULE, CARD_RADIUS, HINT_LINE_HEIGHT,
    MENU_RADIUS, TEXT_BODY, TEXT_HINT, TEXT_SECTION, TEXT_TITLE,
};
pub use devtools_ui::widgets::{icon_button, keycaps, labeled_button, primary_button};

/// 表格行高（卡片高度）。
pub const ROW_HEIGHT: f32 = 36.0;
/// 表格行之间的间隙（卡片之间留白，与剪贴板卡片列表一致）。
pub const ROW_GAP: f32 = 4.0;
/// 表头高度。
pub const TABLE_HEADER_HEIGHT: f32 = 30.0;
/// 顶栏高度。
pub const HEADER_HEIGHT: f32 = 58.0;
/// 状态栏高度。
pub const STATUS_HEIGHT: f32 = 28.0;

/// 标题字体：有 semibold 就用，没有退回常规（字号由调用方决定）。
///
/// 宿主安装的 semibold 字体族来自 `devtools_ui::fonts`；模块不假设它一定存在
/// （例如单独渲染时），检查 `FontDefinitions` 后再用，避免 egui 因未知字体族 panic。
pub fn title_font(ui: &Ui, size: f32) -> FontId {
    let family = FontFamily::Name(devtools_ui::fonts::SEMIBOLD.into());
    let available = ui
        .ctx()
        .fonts(|fonts| fonts.definitions().families.contains_key(&family));
    FontId::new(
        size,
        if available {
            family
        } else {
            FontFamily::Proportional
        },
    )
}

/// 设置分组：标题 + 卡片。
pub fn section<R>(
    ui: &mut Ui,
    palette: &Palette,
    title: &str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new(title)
                .font(title_font(ui, TEXT_SECTION))
                .color(palette.secondary),
        );
    });
    ui.add_space(3.0);
    continuous_frame(
        ui,
        CARD_RADIUS,
        palette.card,
        Stroke::new(1.0, palette.card_stroke),
        egui::Margin::symmetric(12, 6),
        |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            add_contents(ui)
        },
    )
}

/// 左标签、右控件的一行（30 高，控件右对齐）。
pub fn row(ui: &mut Ui, palette: &Palette, label: &str, add_control: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 30.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(RichText::new(label).size(TEXT_BODY).color(palette.text));
            ui.with_layout(Layout::right_to_left(Align::Center), add_control);
        },
    );
}

/// 说明文字（小一号、行距放宽）。
pub fn hint(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(
        RichText::new(text)
            .size(TEXT_HINT)
            .line_height(Some(HINT_LINE_HEIGHT))
            .color(palette.weak),
    );
}

/// 行间分隔线。
pub fn divider(ui: &mut Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range().shrink(2.0),
        rect.center().y,
        Stroke::new(1.0, palette.card_stroke),
    );
}

/// 小号文字胶囊按钮（顶栏的 Aa）。
pub fn pill_button(ui: &mut Ui, palette: &Palette, text: &str, active: bool) -> Response {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), FontId::proportional(11.5), palette.text);
    let width = (galley.size().x + 14.0).max(26.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::click());
    let fill = if active {
        palette.accent.gamma_multiply(0.18)
    } else if response.hovered() {
        palette.field
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CAPSULE, fill);
    let color = if active { palette.accent } else { palette.weak };
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}

/// iOS / macOS 风格的开关（与宿主设置页一致：36×20，滑块带细描边）。
pub fn toggle(ui: &mut Ui, palette: &Palette, on: &mut bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(36.0, 20.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let track = palette.track.lerp_to_gamma(palette.accent, t);
    ui.painter()
        .rect_filled(rect, rect.height() / 2.0, track);
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

/// 小圆点状态指示。
pub fn status_dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

/// 千分位。
pub fn thousands(n: usize) -> String {
    let text = n.to_string();
    let mut out = String::with_capacity(text.len() + text.len() / 3);
    for (i, ch) in text.chars().enumerate() {
        if i > 0 && (text.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 人类可读的大小（1024 进制）。
pub fn byte_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 本地时间：当天显示时分，当年显示月-日，其余显示年月日。
pub fn time_label(ts: Option<i64>) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let Some(ts) = ts else {
        return "—".to_string();
    };
    let Some(dt) = Local.timestamp_opt(ts, 0).single() else {
        return "—".to_string();
    };
    let now = Local::now();
    if dt.date_naive() == now.date_naive() {
        dt.format("%H:%M").to_string()
    } else if dt.year() == now.year() {
        dt.format("%m-%d %H:%M").to_string()
    } else {
        dt.format("%Y-%m-%d").to_string()
    }
}

/// 中间省略（路径列）：保留两头，超宽时中间换成 `…`。
pub fn middle_ellipsis(ui: &Ui, text: &str, font: FontId, width: f32) -> String {
    let measure = |s: &str| {
        ui.painter()
            .layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE)
            .size()
            .x
    };
    if measure(text) <= width || text.chars().count() < 5 {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len() / 2;
    while keep > 1 {
        let head: String = chars[..keep].iter().collect();
        let tail: String = chars[chars.len() - keep..].iter().collect();
        let candidate = format!("{head}…{tail}");
        if measure(&candidate) <= width {
            return candidate;
        }
        keep -= 1;
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_sizes_use_1024() {
        assert_eq!(byte_size(0), "0 B");
        assert_eq!(byte_size(1023), "1023 B");
        assert_eq!(byte_size(1024), "1.0 KB");
        assert_eq!(byte_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(byte_size(200 * 1024), "200 KB");
    }

    #[test]
    fn formats_thousands_and_unknown_time() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(time_label(None), "—");
    }

    #[test]
    fn palettes_come_from_shared_style() {
        let light = Palette::new(false);
        let dark = Palette::new(true);
        assert!(!light.dark && dark.dark);
        assert_eq!(light.background, Color32::from_rgb(246, 246, 248));
        assert_eq!(dark.background, Color32::from_rgb(30, 30, 32));
        assert_eq!(light.accent, dark.accent);
    }
}
