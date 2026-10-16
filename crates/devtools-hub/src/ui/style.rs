//! 字体、配色、圆角与留白规范，以及格式化工具。

use devtools_clipboard::ContentKind;
use egui::{Color32, FontFamily, FontId, Pos2, Rect, Shape, Stroke, TextStyle, Theme, Ui, Vec2};

use super::fonts;
use super::icons::Icon;
use crate::platform::Backdrop;

// 圆角与留白规范，参照 PasteNow（macOS 26 版）：面板窗口圆角 16pt，复制项卡片 15pt
// （选中时 2pt 强调色描边），圆角曲线都是连续曲率（`kCACornerCurveContinuous`，见 [`continuous_rect`]）。
// 全应用只用下面几档：
// - 窗口 16（`panel::CORNER_RADIUS`），内容离窗口边缘 `PAD` = 8；
// - 复制项卡片、预览卡片、设置分组、提示横幅：`CARD_RADIUS`；
// - 按钮、输入框、分类标签、图标按钮：胶囊形 `CAPSULE`；
// - 菜单与提示框 `MENU_RADIUS`，与窗口相同；菜单项是 24 高的胶囊，离外框 `MENU_MARGIN`，与外框同心（见 `menu`）；
// - 键帽、功能徽标、缩略图、复选框等小元素：`SMALL_RADIUS`。

/// 窗口内容到窗口边缘的留白。
pub const PAD: i8 = 8;
/// 卡片、分组：PasteNow 复制项卡片的圆角。
pub const CARD_RADIUS: u8 = 15;
/// 胶囊形：绘制时圆角会被限制为短边的一半。
pub const CAPSULE: u8 = u8::MAX;
/// 菜单、下拉列表与提示框的圆角（`MENU_MARGIN` + 菜单项高度的一半）。
pub const MENU_RADIUS: u8 = 16;
pub const MENU_MARGIN: i8 = 4;
/// 键帽、功能徽标、缩略图、复选框等小元素。
pub const SMALL_RADIUS: u8 = 6;

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts::definitions());
    for theme in [Theme::Light, Theme::Dark] {
        let palette = Palette::new(theme == Theme::Dark, Backdrop::Opaque, 0);
        ctx.style_mut_of(theme, |style| {
            // 字号与 macOS 系统界面接近：正文 13，小字 11。
            style.text_styles = [
                (
                    TextStyle::Heading,
                    FontId::new(17.0, FontFamily::Name(fonts::SEMIBOLD.into())),
                ),
                (TextStyle::Body, FontId::proportional(13.0)),
                (TextStyle::Button, FontId::proportional(13.0)),
                (TextStyle::Monospace, FontId::monospace(12.5)),
                (TextStyle::Small, FontId::proportional(11.0)),
            ]
            .into();
            style.spacing.item_spacing = egui::vec2(6.0, 6.0);
            // macOS 26 标准控件（`NSButton`、`NSPopUpButton`）高 24pt，菜单项也按这个高度。
            style.spacing.interact_size.y = 24.0;
            style.spacing.button_padding = egui::vec2(12.0, 4.0);
            style.spacing.menu_margin = egui::Margin::same(MENU_MARGIN);
            style.interaction.selectable_labels = true;
            let visuals = &mut style.visuals;
            visuals.selection.bg_fill = palette.accent.gamma_multiply(0.35);
            visuals.selection.stroke.color = palette.accent;
            visuals.hyperlink_color = palette.accent;
            visuals.window_corner_radius = MENU_RADIUS.into();
            visuals.menu_corner_radius = MENU_RADIUS.into();
            // 菜单与提示框浮在卡片上方：底色比卡片略亮（浅色模式纯白），细描边加柔和阴影分出层次。
            let dark = theme == Theme::Dark;
            visuals.window_fill = if dark {
                Color32::from_rgb(44, 44, 48)
            } else {
                Color32::WHITE
            };
            visuals.window_stroke = Stroke::new(1.0, palette.border);
            visuals.popup_shadow = egui::Shadow {
                offset: [0, 8],
                blur: 28,
                spread: 0,
                color: Color32::from_black_alpha(if dark { 110 } else { 40 }),
            };
            for widget in [
                &mut visuals.widgets.inactive,
                &mut visuals.widgets.hovered,
                &mut visuals.widgets.active,
                &mut visuals.widgets.open,
                &mut visuals.widgets.noninteractive,
            ] {
                widget.corner_radius = CAPSULE.into();
            }
        });
    }
}

/// 单行输入框。控件是胶囊形，左右留出半个圆角，文字不会贴到弧线上。
pub fn text_field(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(egui::Margin::symmetric(10, 4))
}

/// 连续曲率的圆角矩形（macOS 的 `kCACornerCurveContinuous`，系统窗口和 PasteNow 都用它）。
///
/// 普通圆角是一段圆弧，直边和圆弧交界处曲率突变；连续曲率的圆角从离角 1.53r 处就开始弯，
/// 曲率逐渐变化，看起来更柔和。这里用 iOS 7 起圆角矩形常用的三段三次贝塞尔近似。
/// 描边画在矩形内侧（与 `StrokeKind::Inside` 一致）。
pub fn continuous_rect(rect: Rect, radius: f32, fill: Color32, stroke: Stroke) -> Shape {
    if radius < 0.5 {
        let shape = egui::epaint::RectShape::new(rect, 0, fill, stroke, egui::StrokeKind::Inside);
        return Shape::Rect(shape);
    }
    // 描边以轮廓为中线，轮廓内缩半个线宽，描边就落在原矩形之内。
    let rect = rect.shrink(stroke.width / 2.0);
    // 每个角从离角 a 处开始弯：(a, b) 是相对角点、沿进入边和离开边的距离。
    const START: f32 = 1.528_664_8;
    const CURVES: [[(f32, f32); 3]; 3] = [
        [(1.088_493, 0.0), (0.868_407, 0.0), (0.631_494, 0.074_911)],
        [
            (0.372_824, 0.169_059),
            (0.169_059, 0.372_824),
            (0.074_911, 0.631_494),
        ],
        [(0.0, 0.868_407), (0.0, 1.088_493), (0.0, START)],
    ];
    let radius = radius.min(rect.width().min(rect.height()) / 2.0 / START);
    let steps = (radius / 2.0).ceil().clamp(2.0, 8.0) as usize;
    // 顺时针依次处理右上、右下、左下、左上四个角：角点、指回进入边的方向、沿离开边的方向。
    let corners = [
        (rect.right_top(), Vec2::new(-1.0, 0.0), Vec2::new(0.0, 1.0)),
        (
            rect.right_bottom(),
            Vec2::new(0.0, -1.0),
            Vec2::new(-1.0, 0.0),
        ),
        (
            rect.left_bottom(),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, -1.0),
        ),
        (rect.left_top(), Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)),
    ];
    let mut points = Vec::with_capacity(4 * (1 + 3 * steps));
    for (corner, back, forward) in corners {
        let at = |(a, b): (f32, f32)| corner + back * (a * radius) + forward * (b * radius);
        let mut start = at((START, 0.0));
        points.push(start);
        for [c1, c2, end] in CURVES {
            let (c1, c2, end) = (at(c1), at(c2), at(end));
            for step in 1..=steps {
                let t = step as f32 / steps as f32;
                let u = 1.0 - t;
                let point = start.to_vec2() * (u * u * u)
                    + c1.to_vec2() * (3.0 * u * u * t)
                    + c2.to_vec2() * (3.0 * u * t * t)
                    + end.to_vec2() * (t * t * t);
                points.push(Pos2::ZERO + point);
            }
            start = end;
        }
    }
    Shape::convex_polygon(points, fill, stroke)
}

/// 背景是连续曲率圆角矩形的 `Frame`：先占位，内容画完知道大小后再补上背景。
pub fn continuous_frame<R>(
    ui: &mut Ui,
    radius: u8,
    fill: Color32,
    stroke: Stroke,
    inner_margin: egui::Margin,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let background = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::NONE
        .inner_margin(inner_margin)
        .show(ui, add_contents);
    ui.painter().set(
        background,
        continuous_rect(inner.response.rect, f32::from(radius), fill, stroke),
    );
    inner
}

/// 面板配色。窗口背景是毛玻璃或液态玻璃时底色半透明，让背后的材质透出来。
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub accent: Color32,
    pub background: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub card_stroke: Color32,
    pub field: Color32,
    /// 开关关闭时的轨道颜色。
    pub track: Color32,
    pub warn: Color32,
    pub ok: Color32,
    /// 删除等破坏性操作（macOS 的 systemRed）。
    pub danger: Color32,
}

impl Palette {
    /// `transparency`：设置中的背景透明度（0–100），窗口背景是系统材质时才生效。
    pub fn new(dark: bool, backdrop: Backdrop, transparency: u8) -> Self {
        let accent = Color32::from_rgb(0x30, 0x7b, 0xf6);
        let alpha = match backdrop {
            Backdrop::Opaque => 255,
            Backdrop::Vibrancy | Backdrop::Glass => {
                (255.0 * (1.0 - f32::from(transparency.min(100)) / 100.0)).round() as u8
            }
        };
        let (dark_alpha, light_alpha) = (alpha, alpha);
        if dark {
            Self {
                accent,
                background: Color32::from_rgba_unmultiplied(30, 30, 32, dark_alpha),
                border: Color32::from_white_alpha(30),
                text: Color32::from_rgb(0xf2, 0xf2, 0xf7),
                weak: Color32::from_rgb(0x98, 0x98, 0x9f),
                card: Color32::from_white_alpha(16),
                card_hover: Color32::from_white_alpha(26),
                card_stroke: Color32::from_white_alpha(14),
                field: Color32::from_white_alpha(20),
                track: Color32::from_gray(78),
                warn: Color32::from_rgb(0xff, 0xb3, 0x40),
                ok: Color32::from_rgb(0x34, 0xc7, 0x59),
                danger: Color32::from_rgb(0xff, 0x45, 0x3a),
            }
        } else {
            Self {
                accent,
                background: Color32::from_rgba_unmultiplied(246, 246, 248, light_alpha),
                border: Color32::from_black_alpha(24),
                text: Color32::from_rgb(0x1d, 0x1d, 0x1f),
                weak: Color32::from_rgb(0x86, 0x86, 0x8b),
                card: Color32::from_rgba_unmultiplied(255, 255, 255, 215),
                card_hover: Color32::from_rgba_unmultiplied(255, 255, 255, 245),
                card_stroke: Color32::from_black_alpha(14),
                field: Color32::from_black_alpha(12),
                track: Color32::from_gray(214),
                warn: Color32::from_rgb(0xc2, 0x6a, 0x00),
                ok: Color32::from_rgb(0x28, 0xa7, 0x45),
                danger: Color32::from_rgb(0xff, 0x3b, 0x30),
            }
        }
    }
}

/// 卡片右上角的时间：今天显示时分，昨天显示“昨天”，更早显示日期。
pub fn card_time(ms: i64, now_ms: i64) -> String {
    let (Some(time), Some(now)) = (local(ms), local(now_ms)) else {
        return String::new();
    };
    let days = (now.date_naive() - time.date_naive()).num_days();
    match days {
        0 => time.format("%H:%M").to_string(),
        1 => "昨天".into(),
        _ if time.format("%Y").to_string() == now.format("%Y").to_string() => {
            time.format("%-m月%-d日").to_string()
        }
        _ => time.format("%Y/%-m/%-d").to_string(),
    }
}

/// 2185 → "2,185"
pub fn group_digits(value: i64) -> String {
    let digits = value.abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if value < 0 {
        out.insert(0, '-');
    }
    out
}

pub fn kind_color(kind: ContentKind) -> Color32 {
    match kind {
        ContentKind::Text => Color32::from_rgb(0x6b, 0x7c, 0x93),
        ContentKind::RichText => Color32::from_rgb(0x8e, 0x6c, 0xd8),
        ContentKind::Link => Color32::from_rgb(0x2f, 0x80, 0xed),
        ContentKind::Image => Color32::from_rgb(0x27, 0xae, 0x60),
        ContentKind::Files => Color32::from_rgb(0xe6, 0x7e, 0x22),
        ContentKind::Other => Color32::from_rgb(0x95, 0xa5, 0xa6),
    }
}

pub fn kind_icon(kind: ContentKind) -> Icon {
    match kind {
        ContentKind::Text | ContentKind::RichText => Icon::Document,
        ContentKind::Link => Icon::Link,
        ContentKind::Image => Icon::Image,
        ContentKind::Files => Icon::Folder,
        ContentKind::Other => Icon::Box,
    }
}

pub fn absolute_time(ms: i64) -> String {
    local(ms).map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M:%S").to_string())
}

fn local(ms: i64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&chrono::Local))
}

pub fn byte_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_card_time_and_counts() {
        let now = chrono::Local::now().timestamp_millis();
        assert_eq!(card_time(now, now).len(), 5, "今天显示 HH:MM");
        assert_eq!(card_time(now - 86_400_000, now), "昨天");
        assert_eq!(group_digits(2185), "2,185");
        assert_eq!(group_digits(1_000_000), "1,000,000");
        assert_eq!(group_digits(12), "12");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(byte_size(512), "512 B");
        assert_eq!(byte_size(1536), "1.5 KB");
        assert_eq!(byte_size(5 * 1024 * 1024), "5.0 MB");
    }
}
