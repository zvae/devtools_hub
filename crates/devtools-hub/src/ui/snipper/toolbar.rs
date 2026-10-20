//! 截屏遮罩的工具栏（参照 Snipaste）：
//!
//! - 主工具栏：标注工具（矩形 / 椭圆、箭头、画笔、记号笔、马赛克、文字、橡皮擦）｜撤销、重做｜长截图、提取文字｜
//!   退出、贴图、保存、复制；编辑贴图时没有长截图和提取文字（贴图的右键菜单里有提取文字）；
//!   滚动截图时只有提取文字和输出按钮；
//! - 属性栏：选中标注工具后出现在主工具栏下方（主工具栏在选区上方时出现在它上方），
//!   形状工具可以切换矩形和椭圆，其余是三档粗细（文字是字号）和色板。
//!
//! 与 Snipaste 一样是浅色的圆角条，只有图标，鼠标悬停时显示名称和快捷键。

use egui::{
    pos2, vec2, Color32, CornerRadius, Id, Painter, Pos2, Rect, Sense, Shadow, Stroke, StrokeKind,
    Ui, Vec2,
};

use super::Output;
use crate::keys;
use crate::platform;
use crate::snip::annotation::{Size, Style, Tool, COLORS};
use crate::ui::icons::{self, Icon};

/// 按钮边长（点）。
const BUTTON: f32 = 30.0;
/// 工具栏内边距。
const PAD: f32 = 4.0;
/// 分隔线占的宽度。
const SEPARATOR: f32 = 9.0;
/// 色块边长。
const SWATCH: f32 = 22.0;
/// 工具栏与选区、两个工具栏之间的间距。
const GAP: f32 = 6.0;

const BAR_FILL: Color32 = Color32::from_rgb(250, 250, 250);
const BAR_STROKE: Color32 = Color32::from_rgb(208, 208, 210);
const ICON: Color32 = Color32::from_rgb(58, 58, 62);
const ICON_DISABLED: Color32 = Color32::from_rgb(188, 188, 192);
const HOVER: Color32 = Color32::from_rgb(232, 232, 236);
const ACCENT: Color32 = Color32::from_rgb(0x30, 0x7b, 0xf6);

/// 点击工具栏得到的操作。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum BarAction {
    Tool(Tool),
    Undo,
    Redo,
    /// 进入滚动截图。
    Scroll,
    Output(Output),
    Size(Size),
    Ellipse(bool),
    Color(Color32),
}

/// 工具栏的用途，决定上面有哪些按钮。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// 普通截屏。
    Snip,
    /// 编辑贴图（贴图按钮变成“完成”，没有长截图）。
    Edit,
    /// 滚动截图中：只有输出按钮。
    Scroll,
}

/// 工具栏的当前状态（决定哪个按钮高亮、哪个不可用）。
pub(super) struct BarState {
    pub tool: Option<Tool>,
    pub style: Style,
    pub can_undo: bool,
    pub can_redo: bool,
    pub mode: Mode,
    /// 可以进入滚动截图（没有标注、选区不太小）。
    pub can_scroll: bool,
}

/// 两个工具栏的位置（点）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bars {
    pub main: Rect,
    pub props: Option<Rect>,
}

impl Bars {
    /// 点是否在任意一个工具栏上。
    pub fn contains(&self, point: Pos2) -> bool {
        self.main.contains(point) || self.props.is_some_and(|rect| rect.contains(point))
    }
}

/// 主工具栏上的一项。
#[derive(Clone, Copy)]
enum Item {
    Tool(Tool),
    Undo,
    Redo,
    Scroll,
    Output(Output),
    Separator,
}

/// 编辑贴图时的主工具栏。
const EDIT_ITEMS: [Item; 15] = [
    Item::Tool(Tool::Shape),
    Item::Tool(Tool::Arrow),
    Item::Tool(Tool::Pen),
    Item::Tool(Tool::Marker),
    Item::Tool(Tool::Mosaic),
    Item::Tool(Tool::Text),
    Item::Tool(Tool::Eraser),
    Item::Separator,
    Item::Undo,
    Item::Redo,
    Item::Separator,
    Item::Output(Output::Cancel),
    Item::Output(Output::Pin),
    Item::Output(Output::Save),
    Item::Output(Output::Copy),
];

const SNIP_ITEMS: [Item; 18] = [
    Item::Tool(Tool::Shape),
    Item::Tool(Tool::Arrow),
    Item::Tool(Tool::Pen),
    Item::Tool(Tool::Marker),
    Item::Tool(Tool::Mosaic),
    Item::Tool(Tool::Text),
    Item::Tool(Tool::Eraser),
    Item::Separator,
    Item::Undo,
    Item::Redo,
    Item::Separator,
    Item::Scroll,
    Item::Output(Output::Ocr),
    Item::Separator,
    Item::Output(Output::Cancel),
    Item::Output(Output::Pin),
    Item::Output(Output::Save),
    Item::Output(Output::Copy),
];

/// 不支持滚动截图的平台上普通截屏的工具栏。
const SNIP_NO_SCROLL_ITEMS: [Item; 17] = [
    Item::Tool(Tool::Shape),
    Item::Tool(Tool::Arrow),
    Item::Tool(Tool::Pen),
    Item::Tool(Tool::Marker),
    Item::Tool(Tool::Mosaic),
    Item::Tool(Tool::Text),
    Item::Tool(Tool::Eraser),
    Item::Separator,
    Item::Undo,
    Item::Redo,
    Item::Separator,
    Item::Output(Output::Ocr),
    Item::Separator,
    Item::Output(Output::Cancel),
    Item::Output(Output::Pin),
    Item::Output(Output::Save),
    Item::Output(Output::Copy),
];

const SCROLL_ITEMS: [Item; 6] = [
    Item::Output(Output::Ocr),
    Item::Separator,
    Item::Output(Output::Cancel),
    Item::Output(Output::Pin),
    Item::Output(Output::Save),
    Item::Output(Output::Copy),
];

/// 主工具栏上的按钮。
fn items(mode: Mode) -> &'static [Item] {
    match mode {
        Mode::Snip if platform::SCROLL_CAPTURE_SUPPORTED => &SNIP_ITEMS,
        Mode::Snip => &SNIP_NO_SCROLL_ITEMS,
        Mode::Edit => &EDIT_ITEMS,
        Mode::Scroll => &SCROLL_ITEMS,
    }
}

/// 一组项目的总宽度。
fn width(items: &[Item]) -> f32 {
    let content: f32 = items
        .iter()
        .map(|item| match item {
            Item::Separator => SEPARATOR,
            _ => BUTTON,
        })
        .sum();
    content + PAD * 2.0
}

/// 属性栏上的一项。
#[derive(Clone, Copy)]
enum Prop {
    Shape(bool),
    Size(Size),
    Color(Color32),
    Separator,
}

/// 某个工具的属性栏内容；没有属性的工具（橡皮擦）返回空。
fn props(tool: Tool) -> Vec<Prop> {
    let mut items = Vec::new();
    if tool == Tool::Shape {
        items.extend([Prop::Shape(false), Prop::Shape(true), Prop::Separator]);
    }
    if tool.has_size() {
        items.extend(Size::ALL.map(Prop::Size));
    }
    if tool.has_color() {
        items.push(Prop::Separator);
        items.extend(COLORS.map(Prop::Color));
    }
    items
}

fn props_width(items: &[Prop]) -> f32 {
    let content: f32 = items
        .iter()
        .map(|item| match item {
            Prop::Separator => SEPARATOR,
            Prop::Color(_) => SWATCH + 2.0,
            Prop::Shape(_) | Prop::Size(_) => BUTTON,
        })
        .sum();
    content + PAD * 2.0
}

/// 工具栏的位置：主工具栏在选区右下角下方，放不下时在选区上方，再放不下时在选区里面的右下角；
/// 属性栏贴着主工具栏，在远离选区的一侧。
pub(super) fn layout(selection: Rect, screen: Rect, tool: Option<Tool>, mode: Mode) -> Bars {
    let main_size = vec2(width(items(mode)), BUTTON + PAD * 2.0);
    let props_items = tool.map(props).unwrap_or_default();
    let props_size =
        (!props_items.is_empty()).then(|| vec2(props_width(&props_items), BUTTON + PAD * 2.0));
    let extra = props_size.map_or(0.0, |size| size.y + GAP);
    let total = main_size.y + extra;

    let below = selection.bottom() + GAP;
    let above = selection.top() - GAP - total;
    let (main_y, props_below) = if below + total <= screen.bottom() {
        (below, true)
    } else if above >= screen.top() {
        (above + extra, false)
    } else {
        (selection.bottom() - GAP - total, true)
    };
    // 屏幕比工具栏还窄时靠左（先 min 再 max，避免下限大于上限）。
    let place_x = |w: f32| {
        (selection.right() - w)
            .min(screen.right() - w - 4.0)
            .max(screen.left() + 4.0)
    };
    let main = Rect::from_min_size(pos2(place_x(main_size.x), main_y), main_size);
    let props = props_size.map(|size| {
        let y = if props_below {
            main.bottom() + GAP
        } else {
            main.top() - GAP - size.y
        };
        Rect::from_min_size(pos2(place_x(size.x), y), size)
    });
    Bars { main, props }
}

/// 绘制工具栏，返回点击的操作。
pub(super) fn show(ui: &Ui, painter: &Painter, bars: &Bars, state: &BarState) -> Option<BarAction> {
    let mut action = None;
    paint_bar(painter, bars.main);
    let mut x = bars.main.left() + PAD;
    for (index, item) in items(state.mode).iter().enumerate() {
        if let Item::Separator = item {
            separator(painter, x, bars.main);
            x += SEPARATOR;
            continue;
        }
        let rect = Rect::from_min_size(pos2(x, bars.main.top() + PAD), Vec2::splat(BUTTON));
        x += BUTTON;
        let (icon, hint, enabled, selected, clicked_action) = match *item {
            Item::Tool(tool) => (
                tool_icon(tool),
                tool.label().to_string(),
                true,
                state.tool == Some(tool),
                BarAction::Tool(tool),
            ),
            Item::Undo => (
                Icon::Undo,
                format!("撤销 {}", keys::shortcut(&[keys::COMMAND, "Z"])),
                state.can_undo,
                false,
                BarAction::Undo,
            ),
            Item::Redo => (
                Icon::Redo,
                format!(
                    "重做 {}",
                    keys::shortcut(&[keys::SHIFT, keys::COMMAND, "Z"])
                ),
                state.can_redo,
                false,
                BarAction::Redo,
            ),
            Item::Scroll => (
                Icon::ScrollCapture,
                if state.can_scroll {
                    "长截图（滚动截图）".to_owned()
                } else {
                    "长截图（有标注或选区太小时不可用）".to_owned()
                },
                state.can_scroll,
                false,
                BarAction::Scroll,
            ),
            Item::Output(output) => (
                output_icon(output),
                output_hint(output, state.mode),
                true,
                false,
                BarAction::Output(output),
            ),
            Item::Separator => unreachable!(),
        };
        if button(
            ui,
            painter,
            rect,
            ("snip-bar", index),
            icon,
            &hint,
            enabled,
            selected,
        ) {
            action = Some(clicked_action);
        }
    }

    if let (Some(rect), Some(tool)) = (bars.props, state.tool) {
        paint_bar(painter, rect);
        let mut x = rect.left() + PAD;
        for (index, item) in props(tool).into_iter().enumerate() {
            let id = ("snip-props", index);
            match item {
                Prop::Separator => {
                    separator(painter, x, rect);
                    x += SEPARATOR;
                }
                Prop::Shape(ellipse) => {
                    let button_rect =
                        Rect::from_min_size(pos2(x, rect.top() + PAD), Vec2::splat(BUTTON));
                    x += BUTTON;
                    let (icon, hint) = if ellipse {
                        (Icon::Ellipse, "椭圆")
                    } else {
                        (Icon::Rectangle, "矩形")
                    };
                    let selected = state.style.ellipse == ellipse;
                    if button(ui, painter, button_rect, id, icon, hint, true, selected) {
                        action = Some(BarAction::Ellipse(ellipse));
                    }
                }
                Prop::Size(size) => {
                    let button_rect =
                        Rect::from_min_size(pos2(x, rect.top() + PAD), Vec2::splat(BUTTON));
                    x += BUTTON;
                    if size_button(ui, painter, button_rect, id, tool, size, state.style.size) {
                        action = Some(BarAction::Size(size));
                    }
                }
                Prop::Color(color) => {
                    let swatch = Rect::from_center_size(
                        pos2(x + (SWATCH + 2.0) / 2.0, rect.center().y),
                        Vec2::splat(SWATCH),
                    );
                    x += SWATCH + 2.0;
                    if color_swatch(ui, painter, swatch, id, color, state.style.color == color) {
                        action = Some(BarAction::Color(color));
                    }
                }
            }
        }
    }
    action
}

fn tool_icon(tool: Tool) -> Icon {
    match tool {
        Tool::Shape => Icon::Shapes,
        Tool::Arrow => Icon::Arrow,
        Tool::Pen => Icon::Pencil,
        Tool::Marker => Icon::Marker,
        Tool::Mosaic => Icon::Mosaic,
        Tool::Text => Icon::Text,
        Tool::Eraser => Icon::Eraser,
    }
}

fn output_icon(output: Output) -> Icon {
    match output {
        Output::Cancel => Icon::Close,
        Output::Pin => Icon::Pin,
        Output::Save => Icon::Save,
        Output::Copy => Icon::Copy,
        Output::Ocr => Icon::Ocr,
    }
}

/// 输出按钮的提示：名称与快捷键（与 Snipaste 的 “Pin to screen (Ctrl+T)” 等一致）。
fn output_hint(output: Output, mode: Mode) -> String {
    let command = |key: &str| keys::shortcut(&[keys::COMMAND, key]);
    let editing = mode == Mode::Edit;
    match output {
        Output::Copy if mode == Mode::Scroll => {
            format!("完成并复制 {} / {}", command("C"), keys::RETURN)
        }
        Output::Ocr if mode == Mode::Scroll => format!("完成并提取文字 {}", command("E")),
        Output::Ocr => format!("提取文字 {}", command("E")),
        Output::Cancel if editing => format!("放弃修改 {}", keys::ESCAPE),
        Output::Cancel => format!("退出 {}", keys::ESCAPE),
        Output::Pin if editing => format!("完成编辑 {}", command("T")),
        Output::Pin => format!("贴到屏幕 {}", command("T")),
        Output::Save => format!("保存 {}", command("S")),
        Output::Copy => format!("复制 {} / {} / 双击", command("C"), keys::RETURN),
    }
}

/// 浅色圆角条与阴影。
fn paint_bar(painter: &Painter, rect: Rect) {
    let radius = CornerRadius::same(7);
    painter.add(
        Shadow {
            offset: [0, 2],
            blur: 10,
            spread: 0,
            color: Color32::from_black_alpha(60),
        }
        .as_shape(rect, radius),
    );
    painter.rect(
        rect,
        radius,
        BAR_FILL,
        Stroke::new(1.0, BAR_STROKE),
        StrokeKind::Inside,
    );
}

fn separator(painter: &Painter, x: f32, bar: Rect) {
    let center = x + SEPARATOR / 2.0;
    painter.line_segment(
        [
            pos2(center, bar.top() + 9.0),
            pos2(center, bar.bottom() - 9.0),
        ],
        Stroke::new(1.0, BAR_STROKE),
    );
}

/// 图标按钮，返回是否被点击（不可用时不响应）。
#[allow(clippy::too_many_arguments)]
fn button(
    ui: &Ui,
    painter: &Painter,
    rect: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    icon: Icon,
    hint: &str,
    enabled: bool,
    selected: bool,
) -> bool {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, Id::new(id), sense).on_hover_text(hint);
    let inner = rect.shrink(2.0);
    if selected {
        painter.rect_filled(inner, 5.0, ACCENT.gamma_multiply(0.16));
    } else if enabled && response.hovered() {
        painter.rect_filled(inner, 5.0, HOVER);
    }
    let color = if !enabled {
        ICON_DISABLED
    } else if selected {
        ACCENT
    } else {
        ICON
    };
    icons::paint_centered(painter, rect, 17.0, icon, color);
    enabled && response.clicked()
}

/// 粗细按钮：圆点的大小表示档位。
fn size_button(
    ui: &Ui,
    painter: &Painter,
    rect: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    tool: Tool,
    size: Size,
    current: Size,
) -> bool {
    let hint = match (tool, size) {
        (Tool::Text, Size::Small) => "小号字",
        (Tool::Text, Size::Medium) => "中号字",
        (Tool::Text, Size::Large) => "大号字",
        (_, Size::Small) => "细",
        (_, Size::Medium) => "中",
        (_, Size::Large) => "粗",
    };
    let response = ui
        .interact(rect, Id::new(id), Sense::click())
        .on_hover_text(hint);
    let selected = size == current;
    if selected {
        painter.rect_filled(rect.shrink(2.0), 5.0, ACCENT.gamma_multiply(0.16));
    } else if response.hovered() {
        painter.rect_filled(rect.shrink(2.0), 5.0, HOVER);
    }
    let radius = match size {
        Size::Small => 2.5,
        Size::Medium => 4.0,
        Size::Large => 6.0,
    };
    let color = if selected { ACCENT } else { ICON };
    painter.circle_filled(rect.center(), radius, color);
    response.clicked()
}

/// 色块：选中的颜色外面加一圈强调色。
fn color_swatch(
    ui: &Ui,
    painter: &Painter,
    rect: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    color: Color32,
    selected: bool,
) -> bool {
    let response = ui.interact(rect, Id::new(id), Sense::click());
    let swatch = rect.shrink(if selected { 4.0 } else { 3.0 });
    painter.rect_filled(swatch, 3.0, color);
    painter.rect_stroke(
        swatch,
        3.0,
        Stroke::new(1.0, Color32::from_black_alpha(40)),
        StrokeKind::Inside,
    );
    if selected {
        painter.rect_stroke(rect, 4.0, Stroke::new(1.5, ACCENT), StrokeKind::Inside);
    } else if response.hovered() {
        painter.rect_stroke(rect, 4.0, Stroke::new(1.0, BAR_STROKE), StrokeKind::Inside);
    }
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_stay_on_screen_and_props_go_away_from_selection() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0));
        // 选区上方有空间、下方没有：主工具栏在上，属性栏在它上面。
        let selection = Rect::from_min_max(pos2(600.0, 500.0), pos2(1100.0, 790.0));
        let bars = layout(selection, screen, Some(Tool::Pen), Mode::Snip);
        let props = bars.props.unwrap();
        assert!(bars.main.bottom() <= selection.top());
        assert!(props.bottom() <= bars.main.top());
        assert!(screen.contains_rect(bars.main) && screen.contains_rect(props));
        // 下方有空间：主工具栏在下，属性栏在它下面；右边与选区对齐。
        let selection = Rect::from_min_max(pos2(600.0, 100.0), pos2(1100.0, 300.0));
        let bars = layout(selection, screen, Some(Tool::Shape), Mode::Snip);
        assert!(bars.main.top() >= selection.bottom());
        assert!(bars.props.unwrap().top() >= bars.main.bottom());
        assert_eq!(bars.main.right(), selection.right());
        // 橡皮擦没有属性栏；没有选工具时也没有。
        assert!(layout(selection, screen, Some(Tool::Eraser), Mode::Snip)
            .props
            .is_none());
        assert!(layout(selection, screen, None, Mode::Snip).props.is_none());
        // 屏幕比工具栏还窄：不崩溃，靠左。
        let narrow = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
        let tiny = Rect::from_min_max(pos2(10.0, 10.0), pos2(50.0, 20.0));
        assert_eq!(layout(tiny, narrow, None, Mode::Snip).main.left(), 4.0);
        // 滚动截图的工具栏只有提取文字和四个输出按钮。
        let scroll = layout(selection, screen, None, Mode::Scroll);
        assert_eq!(scroll.main.width(), BUTTON * 5.0 + SEPARATOR + PAD * 2.0);
        assert_eq!(scroll.main.right(), selection.right());
    }
}
