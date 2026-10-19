//! 自绘弹出菜单的排布与绘制，样式参照 macOS 的 `NSMenu`：大圆角底板、细描边和柔和阴影，
//! 悬停的菜单项整行填充强调色的胶囊，右侧是灰色的快捷键或子菜单箭头。
//!
//! Windows 的系统菜单样式与面板不搭，右键菜单改用这里画的菜单（窗口、文字与消息循环见 `windows::menu`）。
//! 这里只做与平台无关的计算，在任何平台上都能测试：菜单项排布、键盘导航、弹出位置，
//! 以及文字以外的像素绘制。坐标与尺寸都是物理像素。
#![cfg_attr(not(windows), allow(dead_code))]

/// 菜单内容。
#[derive(Debug, PartialEq)]
pub enum Node {
    /// 菜单项；`checked` 不为空时是带勾选标记的菜单项。选中后把 `id` 交还给调用方。
    Item {
        label: String,
        shortcut: Option<String>,
        checked: Option<bool>,
        id: usize,
    },
    Submenu {
        label: String,
        children: Vec<Node>,
    },
    Separator,
}

impl Node {
    /// 能高亮、能选中（分隔线不能）。
    pub fn selectable(&self) -> bool {
        !matches!(self, Node::Separator)
    }
}

/// 不透明度未预乘的 sRGB 颜色。
pub type Rgba = [u8; 4];

pub const WHITE: Rgba = [255, 255, 255, 255];

/// 菜单配色，取自面板配色，与面板上的 egui 弹窗一致。
#[derive(Clone, Copy, Debug)]
pub struct Colors {
    /// 底板（不透明）。
    pub fill: Rgba,
    /// 外框描边与分隔线（半透明，叠在底板上）。
    pub border: Rgba,
    pub text: Rgba,
    /// 快捷键文字。
    pub weak: Rgba,
    /// 悬停高亮。
    pub accent: Rgba,
    /// 阴影最深处的颜色。
    pub shadow: Rgba,
}

impl Colors {
    /// 第 `row` 项的文字颜色：（菜单项名字，快捷键）。高亮时都是白色，快捷键略淡。
    pub fn text(&self, highlighted: bool) -> (Rgba, Rgba) {
        if highlighted {
            (WHITE, mix(self.accent, WHITE, 0.85))
        } else {
            (self.text, self.weak)
        }
    }
}

// 尺寸规范（逻辑像素），与面板上的 egui 菜单（`ui::menu`、`ui::style`）一致：
// 外框圆角 16，菜单项是 24 高的胶囊、离外框 4，与外框同心。
const RADIUS: f32 = 16.0;
const MARGIN: f32 = 4.0;
const ROW: f32 = 24.0;
const SEPARATOR: f32 = 9.0;
/// 文字离高亮边缘的距离。
const TEXT_PAD: f32 = 10.0;
/// 勾选标记占的宽度（菜单里有勾选项时才留）。
const CHECK: f32 = 16.0;
/// 菜单项名字与右侧快捷键、箭头之间至少留出的距离。
const GAP: f32 = 28.0;
/// 子菜单箭头的宽、高。
const ARROW: (f32, f32) = (4.5, 8.0);
const MIN_WIDTH: f32 = 140.0;
/// 菜单项名字最多显示的宽度，超出的部分显示省略号（列表名可能很长）。
const MAX_LABEL: f32 = 280.0;
/// 阴影向下偏移的距离与模糊范围，与 egui 弹窗的阴影接近。
const SHADOW_OFFSET: f32 = 6.0;
const SHADOW_BLUR: f32 = 24.0;
/// 描边、分隔线、勾选标记与箭头的线宽。
const LINE: f32 = 1.0;
const MARK_LINE: f32 = 1.5;

/// 整数矩形，右边和下边不含在内。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        (self.left..self.right).contains(&x) && (self.top..self.bottom).contains(&y)
    }

    pub fn offset(&self, x: i32, y: i32) -> Rect {
        Rect {
            left: self.left + x,
            top: self.top + y,
            right: self.right + x,
            bottom: self.bottom + y,
        }
    }
}

/// 一层菜单的排布。窗口四周留出画阴影的空白，菜单底板在窗口中间。
#[derive(Debug)]
pub struct Layout {
    pub scale: f32,
    /// 窗口大小。
    pub width: i32,
    pub height: i32,
    /// 菜单底板在窗口里的位置。
    pub body: Rect,
    /// 每个菜单项占的区域（悬停高亮的胶囊），与菜单内容一一对应。
    pub rows: Vec<Rect>,
    /// 菜单项名字的左边缘与最大宽度（勾选标记画在名字左边）。
    pub label_left: i32,
    pub label_width: i32,
    /// 快捷键与子菜单箭头的右边缘。
    pub text_right: i32,
}

impl Layout {
    /// 按内容排布菜单；`measure` 返回文字的宽度（物理像素）。
    pub fn new(nodes: &[Node], scale: f32, mut measure: impl FnMut(&str) -> i32) -> Self {
        let px = |value: f32| (value * scale).round() as i32;
        let mut label_width = 0;
        let mut shortcut_width = 0;
        let (mut has_check, mut has_arrow) = (false, false);
        for node in nodes {
            match node {
                Node::Item {
                    label,
                    shortcut,
                    checked,
                    ..
                } => {
                    label_width = label_width.max(measure(label));
                    if let Some(shortcut) = shortcut {
                        shortcut_width = shortcut_width.max(measure(shortcut));
                    }
                    has_check |= checked.is_some();
                }
                Node::Submenu { label, .. } => {
                    label_width = label_width.max(measure(label));
                    has_arrow = true;
                }
                Node::Separator => {}
            }
        }
        let label_width = label_width.min(px(MAX_LABEL));
        let right_width = if has_arrow {
            shortcut_width.max(px(ARROW.0))
        } else {
            shortcut_width
        };
        let check = if has_check { px(CHECK) } else { 0 };
        let gap = if right_width > 0 { px(GAP) } else { 0 };
        let body_width =
            (2 * (px(MARGIN) + px(TEXT_PAD)) + check + label_width + gap + right_width)
                .max(px(MIN_WIDTH));

        // 阴影向下偏移，四周统一留出它能到达的最远距离。
        let shadow = px(SHADOW_BLUR / 2.0 + SHADOW_OFFSET);
        let (left, right) = (shadow + px(MARGIN), shadow + body_width - px(MARGIN));
        let mut y = shadow + px(MARGIN);
        let rows = nodes
            .iter()
            .map(|node| {
                let height = px(if node.selectable() { ROW } else { SEPARATOR });
                let row = Rect {
                    left,
                    top: y,
                    right,
                    bottom: y + height,
                };
                y += height;
                row
            })
            .collect();
        let body = Rect {
            left: shadow,
            top: shadow,
            right: shadow + body_width,
            bottom: y + px(MARGIN),
        };
        Self {
            scale,
            width: body.right + shadow,
            height: body.bottom + shadow,
            body,
            rows,
            label_left: left + px(TEXT_PAD) + check,
            label_width,
            text_right: right - px(TEXT_PAD),
        }
    }

    /// 菜单项离外框的距离：子菜单上移这么多，第一项正好与展开它的菜单项对齐。
    pub fn margin(&self) -> i32 {
        (MARGIN * self.scale).round() as i32
    }

    /// 窗口坐标 (x, y) 处可以选中的菜单项。
    pub fn hit(&self, nodes: &[Node], x: i32, y: i32) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.contains(x, y))
            .filter(|&index| nodes[index].selectable())
    }
}

/// 键盘上下移动：从 `from` 往下（`down`）或往上找下一个能选中的菜单项，到头后从另一端继续。
/// 还没有选中项时，往下从第一项开始找，往上从最后一项开始找。
pub fn step(nodes: &[Node], from: Option<usize>, down: bool) -> Option<usize> {
    let len = nodes.len();
    let mut current = from;
    for _ in 0..len {
        let next = match (current, down) {
            (None, true) => 0,
            (None, false) => len - 1,
            (Some(index), true) => (index + 1) % len,
            (Some(index), false) => (index + len - 1) % len,
        };
        if nodes[next].selectable() {
            return Some(next);
        }
        current = Some(next);
    }
    None
}

/// 根菜单的位置：左上角对着鼠标，右边或下边放不下时翻到鼠标左边或上边，最后限制在工作区内。
/// `size` 是菜单底板的大小，返回底板左上角的屏幕坐标。
pub fn place_root(cursor: (i32, i32), size: (i32, i32), work: Rect) -> (i32, i32) {
    let (width, height) = size;
    let mut x = cursor.0;
    if x + width > work.right {
        x = cursor.0 - width;
    }
    let mut y = cursor.1;
    if y + height > work.bottom {
        y = cursor.1 - height;
    }
    (
        clamp(x, work.left, work.right - width),
        clamp(y, work.top, work.bottom - height),
    )
}

/// 子菜单的位置：贴着父菜单的右边，第一项与展开它的菜单项（顶边在 `row_top`）对齐；
/// 右边放不下时放到父菜单左边。`margin` 是菜单项离外框的距离（[`Layout::margin`]）。
pub fn place_submenu(
    parent: Rect,
    row_top: i32,
    margin: i32,
    size: (i32, i32),
    work: Rect,
) -> (i32, i32) {
    let (width, height) = size;
    let mut x = parent.right;
    if x + width > work.right {
        x = parent.left - width;
    }
    (
        clamp(x, work.left, work.right - width),
        clamp(row_top - margin, work.top, work.bottom - height),
    )
}

/// 限制在 `min..=max` 之内；放不下（`max < min`）时贴着 `min`。
fn clamp(value: i32, min: i32, max: i32) -> i32 {
    value.min(max).max(min)
}

/// 窗口像素：每个像素一个 `u32`，按字节依次是 B、G、R、A（32 位 DIB 的格式），左上角为原点。
pub struct Canvas<'a> {
    pub pixels: &'a mut [u32],
    pub width: i32,
    pub height: i32,
}

impl Canvas<'_> {
    /// 在 `area` 内按覆盖率 `coverage(x, y)`（像素中心的坐标）叠一层颜色。
    fn blend(&mut self, area: Rect, color: Rgba, coverage: impl Fn(f32, f32) -> f32) {
        let area = Rect {
            left: area.left.max(0),
            top: area.top.max(0),
            right: area.right.min(self.width),
            bottom: area.bottom.min(self.height),
        };
        let alpha = f32::from(color[3]) / 255.0;
        for y in area.top..area.bottom {
            for x in area.left..area.right {
                let amount = coverage(x as f32 + 0.5, y as f32 + 0.5) * alpha;
                if amount > 0.0 {
                    let index = (y * self.width + x) as usize;
                    let below = unpack(self.pixels[index]);
                    self.pixels[index] = pack(mix(below, color, amount), 255);
                }
            }
        }
    }

    /// 沿折线画一条圆头的线（勾选标记、子菜单箭头）。
    fn polyline(&mut self, points: &[(f32, f32)], width: f32, color: Rgba) {
        let pad = width;
        let (mut area, mut first) = (Rect::default(), true);
        for &(x, y) in points {
            let point = Rect {
                left: (x - pad).floor() as i32,
                top: (y - pad).floor() as i32,
                right: (x + pad).ceil() as i32 + 1,
                bottom: (y + pad).ceil() as i32 + 1,
            };
            area = if first {
                point
            } else {
                Rect {
                    left: area.left.min(point.left),
                    top: area.top.min(point.top),
                    right: area.right.max(point.right),
                    bottom: area.bottom.max(point.bottom),
                }
            };
            first = false;
        }
        self.blend(area, color, |x, y| {
            let distance = points
                .windows(2)
                .map(|pair| segment_distance((x, y), pair[0], pair[1]))
                .fold(f32::MAX, f32::min);
            (width / 2.0 + 0.5 - distance).clamp(0.0, 1.0)
        });
    }
}

/// 画一层菜单里文字以外的内容：底板、高亮、分隔线、勾选标记与子菜单箭头。
/// 这一步只写不透明的颜色，文字画完后再由 [`Frame::apply`] 切出圆角、加上描边和阴影。
pub fn paint(
    canvas: &mut Canvas,
    layout: &Layout,
    nodes: &[Node],
    highlighted: Option<usize>,
    colors: &Colors,
) {
    let scale = layout.scale;
    let fill = pack(colors.fill, 255);
    let body = layout.body;
    for y in body.top.max(0)..body.bottom.min(canvas.height) {
        let start = (y * canvas.width) as usize;
        canvas.pixels[start + body.left as usize..start + body.right as usize].fill(fill);
    }

    for (index, (node, &row)) in nodes.iter().zip(&layout.rows).enumerate() {
        let lit = highlighted == Some(index);
        if lit {
            let (center, half) = rect_geometry(row);
            let radius = half.1;
            canvas.blend(row, colors.accent, |x, y| {
                coverage(rounded_rect_distance((x, y), center, half, radius))
            });
        }
        let (text, _) = colors.text(lit);
        let middle = (row.top + row.bottom) as f32 / 2.0;
        match node {
            Node::Separator => {
                let pad = (TEXT_PAD * scale).round() as i32;
                let line = (LINE * scale).round().max(1.0) as i32;
                let top = row.top + (row.height() - line) / 2;
                let area = Rect {
                    left: row.left + pad,
                    top,
                    right: row.right - pad,
                    bottom: top + line,
                };
                canvas.blend(area, colors.border, |_, _| 1.0);
            }
            Node::Item {
                checked: Some(true),
                ..
            } => {
                // 勾选标记：宽 9、高 7 的对勾，在勾选列里居中。
                let (width, height) = (9.0 * scale, 7.0 * scale);
                let left = layout.label_left as f32 - CHECK * scale + (CHECK * scale - width) / 2.0
                    - 2.0 * scale;
                let top = middle - height / 2.0;
                let points = [
                    (left, top + height * 0.55),
                    (left + width * 0.36, top + height),
                    (left + width, top),
                ];
                canvas.polyline(&points, MARK_LINE * scale, text);
            }
            Node::Submenu { .. } => {
                let (width, height) = (ARROW.0 * scale, ARROW.1 * scale);
                let right = layout.text_right as f32;
                let points = [
                    (right - width, middle - height / 2.0),
                    (right, middle),
                    (right - width, middle + height / 2.0),
                ];
                canvas.polyline(&points, MARK_LINE * scale, text);
            }
            Node::Item { .. } => {}
        }
    }
}

/// 每个像素的外框信息：底板覆盖率、描边覆盖率与阴影不透明度（0–255）。只与排布有关，窗口创建时算一次。
pub struct Frame {
    pixels: Vec<[u8; 3]>,
}

impl Frame {
    pub fn new(layout: &Layout, colors: &Colors) -> Self {
        let scale = layout.scale;
        let (center, half) = rect_geometry(layout.body);
        let radius = RADIUS * scale;
        let line = (LINE * scale).round().max(1.0);
        let blur = SHADOW_BLUR * scale;
        let shadow_center = (center.0, center.1 + SHADOW_OFFSET * scale);
        let shadow_alpha = f32::from(colors.shadow[3]);
        let mut pixels = Vec::with_capacity((layout.width * layout.height) as usize);
        for y in 0..layout.height {
            for x in 0..layout.width {
                let point = (x as f32 + 0.5, y as f32 + 0.5);
                let distance = rounded_rect_distance(point, center, half, radius);
                let cover = coverage(distance);
                // 描边是底板内侧 `line` 宽的一圈。
                let border = cover - coverage(distance + line);
                let shadow = if cover >= 1.0 {
                    0.0
                } else {
                    let distance = rounded_rect_distance(point, shadow_center, half, radius);
                    shadow_alpha * (1.0 - smoothstep(-blur / 2.0, blur / 2.0, distance))
                };
                pixels.push([
                    (cover * 255.0).round() as u8,
                    (border * 255.0).round() as u8,
                    shadow.round() as u8,
                ]);
            }
        }
        Self { pixels }
    }

    /// 在画好内容与文字的像素上切出圆角、加上描边与阴影，转成预乘不透明度的像素
    /// （`UpdateLayeredWindow` 要求预乘）。
    pub fn apply(&self, pixels: &mut [u32], colors: &Colors) {
        let border_alpha = f32::from(colors.border[3]) / 255.0;
        let shadow = colors.shadow;
        for (pixel, &[cover, border, shade]) in pixels.iter_mut().zip(&self.pixels) {
            if cover == 0 {
                let alpha = f32::from(shade) / 255.0;
                *pixel = pack(scale_rgb(shadow, alpha), shade);
                continue;
            }
            let mut color = unpack(*pixel);
            if border > 0 {
                color = mix(
                    color,
                    colors.border,
                    f32::from(border) / 255.0 * border_alpha,
                );
            }
            if cover == 255 {
                *pixel = pack(color, 255);
                continue;
            }
            // 圆角边缘：底板按覆盖率叠在阴影上面。
            let cover = f32::from(cover) / 255.0;
            let shade = f32::from(shade) / 255.0 * (1.0 - cover);
            let mut out = [0u8; 4];
            for channel in 0..3 {
                let value = f32::from(color[channel]) * cover + f32::from(shadow[channel]) * shade;
                out[channel] = value.round().min(255.0) as u8;
            }
            *pixel = pack(out, ((cover + shade) * 255.0).round().min(255.0) as u8);
        }
    }
}

/// `a` 与 `b` 按 `t`（0–1）混合，结果不透明。
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let channel = |i: usize| (f32::from(a[i]) * (1.0 - t) + f32::from(b[i]) * t).round() as u8;
    [channel(0), channel(1), channel(2), 255]
}

fn scale_rgb(color: Rgba, factor: f32) -> Rgba {
    let channel = |i: usize| (f32::from(color[i]) * factor).round() as u8;
    [channel(0), channel(1), channel(2), 255]
}

/// 颜色 → 像素（字节依次是 B、G、R、A）。
pub fn pack(color: Rgba, alpha: u8) -> u32 {
    u32::from_le_bytes([color[2], color[1], color[0], alpha])
}

/// 像素 → 颜色（忽略不透明度）。
pub fn unpack(pixel: u32) -> Rgba {
    let [b, g, r, _] = pixel.to_le_bytes();
    [r, g, b, 255]
}

/// 矩形的中心与半宽、半高。
fn rect_geometry(rect: Rect) -> ((f32, f32), (f32, f32)) {
    let center = (
        (rect.left + rect.right) as f32 / 2.0,
        (rect.top + rect.bottom) as f32 / 2.0,
    );
    (
        center,
        (rect.width() as f32 / 2.0, rect.height() as f32 / 2.0),
    )
}

/// 点到圆角矩形边缘的有符号距离：里面为负，外面为正。
fn rounded_rect_distance(
    point: (f32, f32),
    center: (f32, f32),
    half: (f32, f32),
    radius: f32,
) -> f32 {
    let radius = radius.min(half.0).min(half.1);
    let qx = (point.0 - center.0).abs() - (half.0 - radius);
    let qy = (point.1 - center.1).abs() - (half.1 - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - radius
}

/// 点到线段的距离。
fn segment_distance(point: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 {
        0.0
    } else {
        (((point.0 - a.0) * dx + (point.1 - a.1) * dy) / length).clamp(0.0, 1.0)
    };
    let (x, y) = (a.0 + dx * t - point.0, a.1 + dy * t - point.1);
    (x * x + y * y).sqrt()
}

/// 有符号距离 → 像素被形状覆盖的比例（边缘一个像素宽的抗锯齿）。
fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, shortcut: Option<&str>, id: usize) -> Node {
        Node::Item {
            label: label.into(),
            shortcut: shortcut.map(Into::into),
            checked: None,
            id,
        }
    }

    fn menu() -> Vec<Node> {
        vec![
            item("粘贴", Some("Enter"), 0),
            item("仅复制", Some("Ctrl+Enter"), 1),
            Node::Separator,
            Node::Submenu {
                label: "添加到列表".into(),
                children: vec![Node::Item {
                    label: "收藏".into(),
                    shortcut: None,
                    checked: Some(true),
                    id: 2,
                }],
            },
            Node::Separator,
            item("删除", None, 3),
        ]
    }

    /// 每个字 10 像素宽。
    fn measure(text: &str) -> i32 {
        text.chars().count() as i32 * 10
    }

    #[test]
    fn lays_out_rows_and_columns() {
        let nodes = menu();
        let layout = Layout::new(&nodes, 1.0, measure);
        assert_eq!(layout.rows.len(), nodes.len());
        // 菜单项紧挨着排列，第一项离底板顶边 4，最后一项离底边 4。
        assert_eq!(layout.rows[0].top, layout.body.top + 4);
        for pair in layout.rows.windows(2) {
            assert_eq!(pair[0].bottom, pair[1].top);
        }
        assert_eq!(layout.rows[5].bottom, layout.body.bottom - 4);
        assert_eq!(layout.rows[0].height(), 24);
        assert_eq!(layout.rows[2].height(), 9);
        // 名字最宽 5 个字，快捷键最宽 10 个字符：两侧各 4 + 10，中间留 28。
        assert_eq!(layout.label_width, 50);
        assert_eq!(layout.body.width(), 2 * 14 + 50 + 28 + 100);
        assert_eq!(layout.label_left, layout.body.left + 14);
        assert_eq!(layout.text_right, layout.body.right - 14);
        // 窗口四周留出阴影的空白。
        assert_eq!(layout.width, layout.body.right + layout.body.left);
        assert_eq!(layout.height, layout.body.bottom + layout.body.top);

        // 子菜单里有勾选项：名字右移，留出勾选列；放大两倍时尺寸跟着放大。
        let Node::Submenu { children, .. } = &nodes[3] else {
            unreachable!()
        };
        let submenu = Layout::new(children, 2.0, measure);
        assert_eq!(submenu.label_left, submenu.body.left + 2 * (14 + 16));
        assert_eq!(submenu.body.width(), 280, "内容很窄时用最小宽度");
        assert_eq!(submenu.rows[0].height(), 48);

        // 很长的名字截断到最大宽度。
        let long = vec![item(&"长".repeat(100), None, 0)];
        assert_eq!(Layout::new(&long, 1.0, measure).label_width, 280);
    }

    #[test]
    fn hit_skips_separators() {
        let nodes = menu();
        let layout = Layout::new(&nodes, 1.0, measure);
        let x = layout.body.left + 20;
        let middle = |row: Rect| (row.top + row.bottom) / 2;
        assert_eq!(layout.hit(&nodes, x, middle(layout.rows[1])), Some(1));
        assert_eq!(layout.hit(&nodes, x, middle(layout.rows[2])), None);
        assert_eq!(layout.hit(&nodes, x, middle(layout.rows[3])), Some(3));
        // 底板四周的留白与阴影上没有菜单项。
        assert_eq!(
            layout.hit(&nodes, layout.body.left + 1, middle(layout.rows[1])),
            None
        );
        assert_eq!(layout.hit(&nodes, x, layout.body.top + 1), None);
    }

    #[test]
    fn keyboard_steps_over_separators_and_wraps() {
        let nodes = menu();
        assert_eq!(step(&nodes, None, true), Some(0));
        assert_eq!(step(&nodes, None, false), Some(5));
        assert_eq!(step(&nodes, Some(1), true), Some(3));
        assert_eq!(step(&nodes, Some(3), false), Some(1));
        assert_eq!(step(&nodes, Some(5), true), Some(0));
        assert_eq!(step(&nodes, Some(0), false), Some(5));
        assert_eq!(step(&[Node::Separator], None, true), None);
        assert_eq!(step(&[], None, true), None);
    }

    #[test]
    fn places_menus_inside_work_area() {
        let work = Rect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 800,
        };
        // 放得下：左上角对着鼠标。
        assert_eq!(place_root((100, 100), (200, 300), work), (100, 100));
        // 右边、下边放不下：翻到鼠标左边、上边。
        assert_eq!(place_root((900, 700), (200, 300), work), (700, 400));
        // 翻过去也放不下：贴着工作区边缘。
        assert_eq!(place_root((100, 790), (200, 900), work), (100, 0));

        let parent = Rect {
            left: 100,
            top: 100,
            right: 300,
            bottom: 400,
        };
        // 子菜单贴着父菜单右边，第一项与展开它的菜单项对齐。
        assert_eq!(place_submenu(parent, 200, 4, (150, 100), work), (300, 196));
        // 右边放不下就放到左边。
        let parent = parent.offset(600, 0);
        assert_eq!(place_submenu(parent, 200, 4, (150, 100), work), (550, 196));
        // 下边放不下就往上挪。
        assert_eq!(place_submenu(parent, 780, 4, (150, 100), work), (550, 700));
    }

    #[test]
    fn paints_rounded_body_with_shadow() {
        let nodes = menu();
        let colors = Colors {
            fill: [44, 44, 48, 255],
            border: [255, 255, 255, 30],
            text: [242, 242, 247, 255],
            weak: [152, 152, 159, 255],
            accent: [48, 123, 246, 255],
            shadow: [0, 0, 0, 110],
        };
        let layout = Layout::new(&nodes, 1.0, measure);
        let mut pixels = vec![0u32; (layout.width * layout.height) as usize];
        let mut canvas = Canvas {
            pixels: &mut pixels,
            width: layout.width,
            height: layout.height,
        };
        paint(&mut canvas, &layout, &nodes, Some(1), &colors);
        Frame::new(&layout, &colors).apply(&mut pixels, &colors);
        let at = |x: i32, y: i32| pixels[(y * layout.width + x) as usize].to_le_bytes();
        let row = layout.rows[1];
        let middle = (row.top + row.bottom) / 2;

        // 高亮胶囊中间是强调色，两端是圆的：胶囊外角上还是底色。
        assert_eq!(at(row.left + 30, middle), [246, 123, 48, 255]);
        assert_eq!(at(row.left, row.top), [48, 44, 44, 255]);
        // 未高亮的菜单项是底色。
        assert_eq!(
            at(
                row.left + 30,
                (layout.rows[0].top + layout.rows[0].bottom) / 2
            ),
            [48, 44, 44, 255]
        );
        // 底板的角是圆的：角上透明或只有阴影，没有底色。
        let corner = at(layout.body.left, layout.body.top);
        assert!(corner[3] < 60, "左上角应切掉：{corner:?}");
        // 底板下方是阴影：黑色、半透明，越远越淡，窗口边缘完全透明。
        let below = at(layout.width / 2, layout.body.bottom + 4);
        assert_eq!(below[..3], [0, 0, 0]);
        assert!(below[3] > 20 && below[3] < 110, "{below:?}");
        assert_eq!(at(layout.width / 2, layout.height - 1)[3], 0);
        // 底板边缘有一圈半透明白色描边，比底色亮。
        let edge = at(layout.width / 2, layout.body.top);
        assert!(edge[0] > 48 && edge[3] == 255, "{edge:?}");
    }
}
