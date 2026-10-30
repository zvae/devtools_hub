//! 配色、圆角与留白规范，以及连续曲率圆角等基础绘制工具。
//!
//! 这是各功能模块共用的视觉语言：强调色 `#307bf6`、卡片 15pt 连续曲率圆角、胶囊按钮、
//! 11.5 的说明文字与 13 的正文，明暗两套取值一致。与业务相关的取值（例如剪贴板内容类型
//! 的代表色、时间 / 数字格式化）留在各自的宿主模块里，这里只放平台无关的基础样式。

use egui::{Color32, FontFamily, FontId, Pos2, Rect, Shape, Stroke, TextStyle, Theme, Ui, Vec2};

use crate::fonts;

// 圆角与留白规范，参照 PasteNow（macOS 26 版）：面板窗口圆角 16pt，复制项卡片 15pt
// （选中时 2pt 强调色描边），圆角曲线都是连续曲率（`kCACornerCurveContinuous`，见 [`continuous_rect`]）。
// 全应用只用下面几档：
// - 窗口 16（宿主的 `panel::CORNER_RADIUS`），内容离窗口边缘 `PAD` = 8；
// - 复制项卡片、预览卡片、设置分组、提示横幅：`CARD_RADIUS`；
// - 按钮、输入框、分类标签、图标按钮：胶囊形 `CAPSULE`；
// - 菜单与提示框 `MENU_RADIUS`，与窗口相同；菜单项是 24 高的胶囊，离外框 `MENU_MARGIN`，与外框同心；
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

// 字号规范，全应用只用下面几档：
// - 页面标题 15（`FontFamily::Name(SEMIBOLD)`），分组标题 12；
// - 行标题与正文 13（`TextStyle::Body`）；
// - 说明文字与链接 11.5。
/// 页面标题。
pub const TEXT_TITLE: f32 = 15.0;
/// 分组标题（设置页各分组的“通用”“粘贴”等）。
pub const TEXT_SECTION: f32 = 12.0;
/// 行标题与正文。
pub const TEXT_BODY: f32 = 13.0;
/// 说明文字与链接。
pub const TEXT_HINT: f32 = 11.5;
/// 说明文字的行距。egui 按字体族的主字体（SF Pro，约 1.18em）逐个行框排行，
/// 中文字形比西文高（苹方约 1.4em），默认行距下中文换行会挤在一起；
/// 这里给到 11.5 × 1.4 ≈ 16，与系统界面的中文行距接近。
pub const HINT_LINE_HEIGHT: f32 = 16.0;

/// 安装字体，并为明暗两套主题设置字号、间距、圆角与控件颜色。
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts::definitions());
    for theme in [Theme::Light, Theme::Dark] {
        let palette = Palette::new(theme == Theme::Dark);
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
                // 文字一律用配色里的主文字色。egui 默认给的是普通文字 gray(140)、
                // 按钮文字 gray(180)（浅色模式下 gray(80)、gray(60)），比配色里的
                // 说明文字（`Palette::weak`）还淡，行标题会比说明文字更弱，层级颠倒。
                widget.fg_stroke.color = palette.text;
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

/// 面板配色。窗口背景是毛玻璃或液态玻璃时底色半透明，让背后的材质透出来（见 [`Palette::with_alpha`]）。
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// 当前是否是深色主题。
    pub dark: bool,
    pub accent: Color32,
    pub background: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    /// 结构性标签（分组标题、表头）：比 weak 实一档，避免在浅底上“发虚”。
    pub secondary: Color32,
    /// 更淡的辅助信息（键帽旁的说明、占位提示）。
    pub faint: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub card_stroke: Color32,
    pub field: Color32,
    /// 开关关闭时的轨道颜色。
    pub track: Color32,
    pub warn: Color32,
    pub ok: Color32,
    pub danger: Color32,
}

impl Palette {
    /// 不透明底色（alpha 255）的配色：常规窗口、离屏渲染与单独绘制控件时用。
    pub fn new(dark: bool) -> Self {
        Self::with_alpha(dark, 255)
    }

    /// `alpha`：窗口背景色的不透明度（0–255）。窗口背景是系统材质（毛玻璃 / 液态玻璃）时
    /// 用半透明底色让背后的材质透出来；宿主按窗口材质与设置里的背景透明度换算后传入。
    pub fn with_alpha(dark: bool, alpha: u8) -> Self {
        let accent = Color32::from_rgb(0x30, 0x7b, 0xf6);
        if dark {
            Self {
                dark,
                accent,
                background: Color32::from_rgba_unmultiplied(30, 30, 32, alpha),
                border: Color32::from_white_alpha(30),
                text: Color32::from_rgb(0xf2, 0xf2, 0xf7),
                weak: Color32::from_rgb(0x98, 0x98, 0x9f),
                secondary: Color32::from_rgb(0xb4, 0xb4, 0xbc),
                faint: Color32::from_rgb(0x6e, 0x6e, 0x76),
                card: Color32::from_white_alpha(16),
                card_hover: Color32::from_white_alpha(26),
                card_stroke: Color32::from_white_alpha(14),
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
                background: Color32::from_rgba_unmultiplied(246, 246, 248, alpha),
                border: Color32::from_black_alpha(24),
                text: Color32::from_rgb(0x1d, 0x1d, 0x1f),
                weak: Color32::from_rgb(0x86, 0x86, 0x8b),
                secondary: Color32::from_rgb(0x5c, 0x5c, 0x63),
                faint: Color32::from_rgb(0x9a, 0x9a, 0xa0),
                card: Color32::from_rgba_unmultiplied(255, 255, 255, 215),
                card_hover: Color32::from_rgba_unmultiplied(255, 255, 255, 245),
                card_stroke: Color32::from_black_alpha(14),
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
