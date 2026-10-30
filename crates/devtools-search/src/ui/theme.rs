//! 视觉规范：配色、圆角、排版与通用小控件。
//!
//! 与宿主剪贴板面板 / 设置页共用同一套语言（`crates/devtools-hub/src/ui/style.rs`）：
//! 强调色 `#307bf6`、卡片 15pt 连续曲率圆角、胶囊按钮、11.5 的说明文字与 13 的正文；
//! 明暗两套取值与宿主一致，搜索页是不透明底（§5.2）。

use egui::{
    Align, Color32, FontFamily, FontId, Layout, Pos2, Rect, Response, RichText, Sense, Shape,
    Stroke, StrokeKind, Ui, Vec2,
};

use super::icons::{self, Icon};

/// 卡片 / 分组圆角（与宿主 `style::CARD_RADIUS` 一致）。
pub const CARD_RADIUS: u8 = 15;
/// 胶囊形（绘制时会被限制为短边的一半）。
pub const CAPSULE: u8 = u8::MAX;
/// 键帽、缩略图等小元素。
pub const SMALL_RADIUS: u8 = 6;
/// 菜单 / 历史下拉的圆角（与宿主一致）。
pub const MENU_RADIUS: u8 = 15;

pub const TEXT_TITLE: f32 = 16.0;
pub const TEXT_BODY: f32 = 13.0;
pub const TEXT_SECTION: f32 = 13.0;
pub const TEXT_HINT: f32 = 11.5;
pub const HINT_LINE_HEIGHT: f32 = 16.0;

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

/// 配色（与宿主 `Palette` 的取值一致；搜索页不透明，不带透明度）。
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub accent: Color32,
    pub background: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    /// 结构性标签（分组标题、表头）：比 weak 实一档，避免在浅底上"发虚"。
    pub secondary: Color32,
    pub faint: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub card_stroke: Color32,
    pub field: Color32,
    pub track: Color32,
    pub warn: Color32,
    pub ok: Color32,
    pub danger: Color32,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        let accent = Color32::from_rgb(0x30, 0x7b, 0xf6);
        if dark {
            Self {
                dark,
                accent,
                background: Color32::from_rgb(30, 30, 32),
                border: Color32::from_white_alpha(30),
                text: Color32::from_rgb(0xf2, 0xf2, 0xf7),
                weak: Color32::from_rgb(0x98, 0x98, 0x9f),
                secondary: Color32::from_rgb(0xb4, 0xb4, 0xbc),
                faint: Color32::from_rgb(0x6e, 0x6e, 0x76),
                card: Color32::from_white_alpha(20),
                card_hover: Color32::from_white_alpha(32),
                card_stroke: Color32::from_white_alpha(18),
                field: Color32::from_white_alpha(20),
                track: Color32::from_gray(78),
                warn: Color32::from_rgb(0xff, 0xb3, 0x40),
                ok: Color32::from_rgb(0x34, 0xc7, 0x59),
                danger: Color32::from_rgb(0xff, 0x6b, 0x63),
            }
        } else {
            Self {
                dark,
                accent,
                background: Color32::from_rgb(246, 246, 248),
                border: Color32::from_black_alpha(24),
                text: Color32::from_rgb(0x1d, 0x1d, 0x1f),
                weak: Color32::from_rgb(0x86, 0x86, 0x8b),
                secondary: Color32::from_rgb(0x5c, 0x5c, 0x63),
                faint: Color32::from_rgb(0x9a, 0x9a, 0xa0),
                card: Color32::WHITE,
                card_hover: Color32::WHITE,
                card_stroke: Color32::from_black_alpha(16),
                field: Color32::from_black_alpha(12),
                track: Color32::from_gray(214),
                warn: Color32::from_rgb(0xc2, 0x6a, 0x00),
                ok: Color32::from_rgb(0x28, 0xa7, 0x45),
                danger: Color32::from_rgb(0xd7, 0x00, 0x15),
            }
        }
    }
}

/// 按当前主题取配色。
pub fn palette(ui: &Ui) -> Palette {
    Palette::new(ui.visuals().dark_mode)
}

/// 搜索页的窗口底色（模块与宿主共用，保证圆角窗口与内容底色一致）。
pub fn page_background(dark: bool) -> Color32 {
    Palette::new(dark).background
}

/// 连续曲率的圆角矩形（与宿主 `style::continuous_rect` 相同的三段贝塞尔近似）。
pub fn continuous_rect(rect: Rect, radius: f32, fill: Color32, stroke: Stroke) -> Shape {
    if radius < 0.5 {
        return Shape::Rect(egui::epaint::RectShape::new(
            rect,
            0,
            fill,
            stroke,
            StrokeKind::Inside,
        ));
    }
    let rect = rect.shrink(stroke.width / 2.0);
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

/// 背景是连续曲率圆角矩形的 `Frame`：先占位，画完内容再补背景。
pub fn card_frame<R>(
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

/// 宿主安装的 semibold 字体族（与剪贴板面板 / 设置页的分组标题、页面标题同款）。
///
/// 宿主 `style::install` 会注册名为 `semibold` 的字体族；模块不假设它一定存在
/// （例如单独渲染时），检查 `FontDefinitions` 后再用，避免 egui 因未知字体族 panic。
pub fn semibold_family(ctx: &egui::Context) -> Option<FontFamily> {
    let family = FontFamily::Name("semibold".into());
    ctx.fonts(|fonts| fonts.definitions().families.contains_key(&family))
        .then_some(family)
}

/// 标题字体：有 semibold 就用，没有退回常规（字号由调用方决定）。
pub fn title_font(ui: &Ui, size: f32) -> FontId {
    let family = semibold_family(ui.ctx()).unwrap_or(FontFamily::Proportional);
    FontId::new(size, family)
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
    card_frame(
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

/// 图标按钮（悬停浅底，`active` 用强调色）。
pub fn icon_button(ui: &mut Ui, palette: &Palette, icon: Icon, active: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
    if response.hovered() || active {
        let fill = if active {
            palette.accent.gamma_multiply(0.16)
        } else {
            palette.field
        };
        ui.painter().rect_filled(rect, CAPSULE, fill);
    }
    let color = if active { palette.accent } else { palette.weak };
    icons::paint_centered(ui.painter(), rect, 14.0, icon, color);
    response
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

/// 图标 + 文字的胶囊按钮（`primary` 用强调色底、白字）。
pub fn labeled_button(
    ui: &mut Ui,
    palette: &Palette,
    icon: Option<Icon>,
    label: &str,
    primary: bool,
) -> Response {
    let font = FontId::proportional(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, palette.text);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let width = galley.size().x + icon_w + 26.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
    let (fill, text_color, icon_color) = if primary {
        let fill = if response.hovered() {
            palette.accent.gamma_multiply(0.92)
        } else {
            palette.accent
        };
        (fill, Color32::WHITE, Color32::WHITE)
    } else if response.hovered() {
        (palette.field, palette.text, palette.weak)
    } else {
        (palette.card, palette.text, palette.weak)
    };
    ui.painter().rect_filled(rect, CAPSULE, fill);
    if !primary {
        ui.painter().rect_stroke(
            rect,
            CAPSULE,
            Stroke::new(1.0, palette.card_stroke),
            StrokeKind::Inside,
        );
    }
    let mut x = rect.left() + 13.0;
    if let Some(icon) = icon {
        let icon_rect =
            Rect::from_center_size(egui::pos2(x + 9.0, rect.center().y), Vec2::splat(13.0));
        icons::paint(ui.painter(), icon_rect, icon, icon_color);
        x += icon_w;
    }
    ui.painter().galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        text_color,
    );
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

/// 键帽提示（`↵`、`⌘R` 之类），与宿主底栏的键帽同款。
pub fn keycaps(ui: &mut Ui, palette: &Palette, keys: &[&str]) {
    const HEIGHT: f32 = 19.0;
    const GAP: f32 = 3.0;
    let galleys: Vec<_> = keys
        .iter()
        .map(|key| match *key {
            "↵" => None,
            other => Some(ui.painter().layout_no_wrap(
                other.to_string(),
                FontId::proportional(11.0),
                palette.weak,
            )),
        })
        .collect();
    let widths: Vec<f32> = galleys
        .iter()
        .map(|g| g.as_ref().map_or(HEIGHT, |g| (g.size().x + 10.0).max(HEIGHT)))
        .collect();
    let total = widths.iter().sum::<f32>() + GAP * widths.len().saturating_sub(1) as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, HEIGHT), Sense::hover());
    let mut x = rect.left();
    for ((key, galley), width) in keys.iter().zip(galleys).zip(widths) {
        let cap = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(width, HEIGHT));
        ui.painter().rect(
            cap,
            SMALL_RADIUS,
            palette.card,
            Stroke::new(1.0, palette.border),
            StrokeKind::Inside,
        );
        match (&galley, *key) {
            (Some(g), _) => {
                ui.painter()
                    .galley(cap.center() - g.size() / 2.0, g.clone(), palette.weak);
            }
            (None, _) => {
                icons::paint_centered(ui.painter(), cap, 12.0, Icon::Return, palette.weak)
            }
        }
        x += width + GAP;
    }
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
    fn palettes_have_expected_surfaces() {
        let light = Palette::new(false);
        let dark = Palette::new(true);
        assert_eq!(light.background, Color32::from_rgb(246, 246, 248));
        assert_eq!(dark.background, Color32::from_rgb(30, 30, 32));
        assert_eq!(light.accent, dark.accent);
    }
}
