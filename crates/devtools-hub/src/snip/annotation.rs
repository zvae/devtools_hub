//! 截屏标注（参照 Snipaste 的 `PaintScene`：`RectEllipsePaintItem`、`ArrowPaintItem`、`PencilPaintItem`、
//! `HightlightPaintItem`、`MosaicPaintItem`、`TextPaintItem`、`EraserPaintItem`，撤销与重做）。
//!
//! 坐标与线宽都用截图的像素，与选区一致。这里只有数据和计算（命中测试、马赛克格子、撤销重做），
//! 界面（`ui::snipper`）负责绘制；输出时把截屏遮罩的画面（截图 + 标注）截下来，见那里的“导出”。

use std::collections::HashSet;

use egui::{pos2, vec2, Color32, Pos2, Rect, Vec2};
use image::RgbaImage;

/// 标注工具（工具栏上从左到右的顺序，与 Snipaste 一致）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// 矩形或椭圆。
    Shape,
    Arrow,
    /// 画笔。
    Pen,
    /// 记号笔（半透明的粗笔）。
    Marker,
    /// 马赛克（笔刷）。
    Mosaic,
    Text,
    /// 橡皮擦：擦掉碰到的标注。
    Eraser,
}

impl Tool {
    /// 工具栏上的提示。
    pub fn label(self) -> &'static str {
        match self {
            Tool::Shape => "矩形 / 椭圆",
            Tool::Arrow => "箭头",
            Tool::Pen => "画笔",
            Tool::Marker => "记号笔",
            Tool::Mosaic => "马赛克",
            Tool::Text => "文字",
            Tool::Eraser => "橡皮擦",
        }
    }

    /// 是否可以选颜色（马赛克、橡皮擦没有颜色）。
    pub fn has_color(self) -> bool {
        !matches!(self, Tool::Mosaic | Tool::Eraser)
    }

    /// 是否可以选粗细（文字工具是字号）。
    pub fn has_size(self) -> bool {
        self != Tool::Eraser
    }
}

/// 粗细档位（文字工具是字号）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Small,
    Medium,
    Large,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::Small, Size::Medium, Size::Large];

    /// 某个工具在这一档的线宽（文字工具是字号），单位是逻辑点；画到截图上时乘以缩放比例。
    pub fn points(self, tool: Tool) -> f32 {
        let [small, medium, large] = match tool {
            Tool::Shape | Tool::Arrow | Tool::Pen => [2.0, 4.0, 7.0],
            Tool::Marker => [12.0, 18.0, 26.0],
            Tool::Mosaic => [14.0, 24.0, 40.0],
            Tool::Text => [14.0, 20.0, 30.0],
            // 橡皮擦的判定范围。
            Tool::Eraser => [8.0, 8.0, 8.0],
        };
        match self {
            Size::Small => small,
            Size::Medium => medium,
            Size::Large => large,
        }
    }
}

/// 色板。第一个是默认颜色（与 Snipaste 默认的红色一致）。
pub const COLORS: [Color32; 8] = [
    Color32::from_rgb(0xF5, 0x3A, 0x3A),
    Color32::from_rgb(0xFF, 0x9F, 0x0A),
    Color32::from_rgb(0xFF, 0xD6, 0x0A),
    Color32::from_rgb(0x30, 0xC7, 0x58),
    Color32::from_rgb(0x0A, 0x84, 0xFF),
    Color32::from_rgb(0xAF, 0x52, 0xDE),
    Color32::from_rgb(0x1C, 0x1C, 0x1E),
    Color32::WHITE,
];

/// 记号笔的不透明度。
const MARKER_ALPHA: u8 = 110;

/// 当前选择的颜色、粗细与形状。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub color: Color32,
    pub size: Size,
    /// 形状工具画椭圆（否则是矩形）。
    pub ellipse: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color: COLORS[0],
            size: Size::Medium,
            ellipse: false,
        }
    }
}

impl Style {
    /// 记号笔用的半透明颜色。
    pub fn marker_color(self) -> Color32 {
        let [r, g, b, _] = self.color.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, MARKER_ALPHA)
    }
}

/// 一个标注（像素坐标）。
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Shape {
        rect: Rect,
        ellipse: bool,
        color: Color32,
        width: f32,
    },
    Arrow {
        from: Pos2,
        to: Pos2,
        color: Color32,
        width: f32,
    },
    /// 画笔与记号笔的笔迹（记号笔的颜色是半透明的）。
    Stroke {
        points: Vec<Pos2>,
        color: Color32,
        width: f32,
    },
    Mosaic(Mosaic),
    Text {
        /// 左上角。
        pos: Pos2,
        text: String,
        color: Color32,
        /// 字号（像素）。
        size: f32,
        /// 自动换行的宽度（像素）：从左上角到选区右边缘。
        wrap: f32,
        /// 排版后的大小（像素），橡皮擦用来判断是否碰到。
        extent: Vec2,
    },
}

impl Mark {
    /// 点是否碰到了标注（橡皮擦用），`tolerance` 是额外的判定距离（像素）。
    pub fn hit(&self, point: Pos2, tolerance: f32) -> bool {
        match self {
            Mark::Shape {
                rect,
                ellipse,
                width,
                ..
            } => {
                let reach = width / 2.0 + tolerance;
                if *ellipse {
                    let offset = point - rect.center();
                    let radius = (rect.size() / 2.0).max(Vec2::splat(1.0));
                    let k = ((offset.x / radius.x).powi(2) + (offset.y / radius.y).powi(2)).sqrt();
                    if k < 1e-3 {
                        return radius.min_elem() <= reach;
                    }
                    // 沿着从中心出发的射线，点到椭圆边的距离。
                    let distance = offset.length();
                    (distance - distance / k).abs() <= reach
                } else {
                    let inner = rect.shrink(reach);
                    rect.expand(reach).contains(point)
                        && !(inner.is_positive() && inner.contains(point))
                }
            }
            Mark::Arrow {
                from, to, width, ..
            } => {
                let (head, _) = arrow_head(*from, *to, *width);
                distance_to_segment(point, *from, *to) <= width / 2.0 + tolerance
                    || distance_to_segment(point, head[1], head[2]) <= tolerance
                    || distance_to_segment(point, head[0], head[1]) <= tolerance
                    || distance_to_segment(point, head[0], head[2]) <= tolerance
            }
            Mark::Stroke { points, width, .. } => {
                distance_to_polyline(point, points) <= width / 2.0 + tolerance
            }
            Mark::Mosaic(mosaic) => {
                distance_to_polyline(point, &mosaic.points) <= mosaic.width / 2.0 + tolerance
            }
            Mark::Text { pos, extent, .. } => Rect::from_min_size(*pos, *extent)
                .expand(tolerance)
                .contains(point),
        }
    }
}

/// 马赛克笔迹：笔刷经过的格子用格子内的平均颜色填满（格子对齐到截图的网格，重叠的地方不会更花）。
#[derive(Clone, Debug, PartialEq)]
pub struct Mosaic {
    pub points: Vec<Pos2>,
    /// 笔刷直径（像素）。
    pub width: f32,
    /// 格子边长（像素）。
    pub block: f32,
    /// 已经填上的格子与颜色。
    pub cells: Vec<(Rect, Color32)>,
    seen: HashSet<(i32, i32)>,
}

impl Mosaic {
    /// 在 `start` 开始一笔马赛克。
    pub fn new(image: &RgbaImage, start: Pos2, width: f32, block: f32) -> Self {
        let mut mosaic = Self {
            points: Vec::new(),
            width,
            block: block.max(2.0),
            cells: Vec::new(),
            seen: HashSet::new(),
        };
        mosaic.extend(image, start);
        mosaic
    }

    /// 笔迹延伸到 `point`：把新的一段经过的格子加进来。
    pub fn extend(&mut self, image: &RgbaImage, point: Pos2) {
        let from = self.points.last().copied().unwrap_or(point);
        self.points.push(point);
        let radius = self.width / 2.0;
        let block = self.block;
        let bounds = Rect::from_min_size(
            Pos2::ZERO,
            vec2(image.width() as f32, image.height() as f32),
        );
        let min = from.min(point) - Vec2::splat(radius);
        let max = from.max(point) + Vec2::splat(radius);
        let (x0, y0) = (
            (min.x / block).floor() as i32,
            (min.y / block).floor() as i32,
        );
        let (x1, y1) = ((max.x / block).ceil() as i32, (max.y / block).ceil() as i32);
        for gy in y0..y1 {
            for gx in x0..x1 {
                if self.seen.contains(&(gx, gy)) {
                    continue;
                }
                let cell = Rect::from_min_size(
                    pos2(gx as f32 * block, gy as f32 * block),
                    Vec2::splat(block),
                );
                if distance_to_segment(cell.center(), from, point) > radius {
                    continue;
                }
                let cell = cell.intersect(bounds);
                if !cell.is_positive() {
                    continue;
                }
                self.seen.insert((gx, gy));
                self.cells.push((cell, average_color(image, cell)));
            }
        }
    }
}

/// 图像中一块区域的平均颜色（不透明）。
fn average_color(image: &RgbaImage, rect: Rect) -> Color32 {
    let (x0, y0) = (rect.min.x as u32, rect.min.y as u32);
    let (x1, y1) = (
        (rect.max.x as u32).min(image.width()),
        (rect.max.y as u32).min(image.height()),
    );
    let mut sum = [0u64; 3];
    let mut count = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            sum[0] += u64::from(r);
            sum[1] += u64::from(g);
            sum[2] += u64::from(b);
            count += 1;
        }
    }
    let count = count.max(1);
    Color32::from_rgb(
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    )
}

/// 箭头：返回三角形箭头的三个顶点（尖端在前）和箭杆的终点（三角形底边中点）。
pub fn arrow_head(from: Pos2, to: Pos2, width: f32) -> ([Pos2; 3], Pos2) {
    let direction = to - from;
    let length = direction.length();
    if length < 1e-3 {
        return ([to; 3], to);
    }
    let unit = direction / length;
    let normal = vec2(-unit.y, unit.x);
    let head = (width * 3.5 + 8.0).min(length);
    let base = to - unit * head;
    let half = head * 0.45;
    ([to, base + normal * half, base - normal * half], base)
}

/// 点到线段的距离。
pub fn distance_to_segment(point: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let length_sq = ab.length_sq();
    if length_sq < 1e-6 {
        return (point - a).length();
    }
    let t = ((point - a).dot(ab) / length_sq).clamp(0.0, 1.0);
    (point - (a + ab * t)).length()
}

/// 点到折线的距离（只有一个点时是到这个点的距离）。
pub fn distance_to_polyline(point: Pos2, points: &[Pos2]) -> f32 {
    match points {
        [] => f32::INFINITY,
        [only] => (point - *only).length(),
        _ => points
            .windows(2)
            .map(|pair| distance_to_segment(point, pair[0], pair[1]))
            .fold(f32::INFINITY, f32::min),
    }
}

/// 一次截屏的全部标注，带撤销与重做（每一步保存一份完整的列表，标注不多，复制很便宜）。
#[derive(Clone, Debug, Default)]
pub struct Marks {
    items: Vec<Mark>,
    undo: Vec<Vec<Mark>>,
    redo: Vec<Vec<Mark>>,
}

impl Marks {
    pub fn items(&self) -> &[Mark] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 加一个标注。
    pub fn push(&mut self, mark: Mark) {
        self.checkpoint();
        self.items.push(mark);
    }

    /// 擦掉点上最上层的标注，返回是否擦掉了。
    pub fn erase_at(&mut self, point: Pos2, tolerance: f32) -> bool {
        let Some(index) = self
            .items
            .iter()
            .rposition(|mark| mark.hit(point, tolerance))
        else {
            return false;
        };
        self.checkpoint();
        self.items.remove(index);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// 撤销上一步，返回是否有可撤销的操作。
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.items, previous));
        true
    }

    /// 重做撤销掉的一步。
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.items, next));
        true
    }

    /// 修改之前保存当前状态；新的修改让重做失效。
    fn checkpoint(&mut self) {
        self.undo.push(self.items.clone());
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn stroke(points: &[(f32, f32)], width: f32) -> Mark {
        Mark::Stroke {
            points: points.iter().map(|&(x, y)| pos2(x, y)).collect(),
            color: COLORS[0],
            width,
        }
    }

    #[test]
    fn mosaic_fills_cells_along_stroke_with_average_colors() {
        // 左半边红、右半边蓝。
        let mut image = RgbaImage::from_pixel(40, 20, Rgba([255, 0, 0, 255]));
        for y in 0..20 {
            for x in 20..40 {
                image.put_pixel(x, y, Rgba([0, 0, 255, 255]));
            }
        }
        let mut mosaic = Mosaic::new(&image, pos2(5.0, 5.0), 10.0, 10.0);
        mosaic.extend(&image, pos2(35.0, 5.0));
        let first_row: Vec<_> = mosaic
            .cells
            .iter()
            .filter(|(cell, _)| cell.min.y == 0.0)
            .collect();
        assert_eq!(first_row.len(), 4);
        for (cell, color) in first_row {
            let expected = if cell.min.x < 20.0 {
                Color32::from_rgb(255, 0, 0)
            } else {
                Color32::from_rgb(0, 0, 255)
            };
            assert_eq!(*color, expected);
        }
        // 来回涂同一处不会重复加格子。
        let count = mosaic.cells.len();
        mosaic.extend(&image, pos2(5.0, 5.0));
        assert_eq!(mosaic.cells.len(), count);
        // 格子不超出图像。
        assert!(mosaic
            .cells
            .iter()
            .all(|(cell, _)| cell.min.x >= 0.0 && cell.max.x <= 40.0 && cell.max.y <= 20.0));
    }

    #[test]
    fn hit_tests_follow_shape_outlines() {
        let rect = Rect::from_min_max(pos2(10.0, 10.0), pos2(50.0, 30.0));
        let shape = Mark::Shape {
            rect,
            ellipse: false,
            color: COLORS[0],
            width: 2.0,
        };
        assert!(shape.hit(pos2(10.0, 20.0), 2.0));
        assert!(!shape.hit(pos2(30.0, 20.0), 2.0), "矩形中间是空的");
        let ellipse = Mark::Shape {
            rect,
            ellipse: true,
            color: COLORS[0],
            width: 2.0,
        };
        assert!(ellipse.hit(pos2(50.0, 20.0), 1.0));
        assert!(ellipse.hit(pos2(30.0, 10.5), 1.0));
        assert!(!ellipse.hit(pos2(30.0, 20.0), 1.0));
        assert!(
            !ellipse.hit(pos2(12.0, 12.0), 1.0),
            "外接矩形的角上不在椭圆上"
        );

        let line = stroke(&[(0.0, 0.0), (100.0, 0.0)], 4.0);
        assert!(line.hit(pos2(50.0, 3.0), 2.0));
        assert!(!line.hit(pos2(50.0, 6.0), 2.0));

        let text = Mark::Text {
            pos: pos2(5.0, 5.0),
            text: "你好".into(),
            color: COLORS[0],
            size: 20.0,
            wrap: 100.0,
            extent: vec2(40.0, 24.0),
        };
        assert!(text.hit(pos2(30.0, 20.0), 0.0));
        assert!(!text.hit(pos2(60.0, 20.0), 0.0));

        let arrow = Mark::Arrow {
            from: pos2(0.0, 0.0),
            to: pos2(100.0, 0.0),
            color: COLORS[0],
            width: 4.0,
        };
        assert!(arrow.hit(pos2(30.0, 1.0), 1.0));
        assert!(!arrow.hit(pos2(30.0, 10.0), 1.0));
    }

    #[test]
    fn arrow_head_points_at_target() {
        let (head, base) = arrow_head(pos2(0.0, 0.0), pos2(100.0, 0.0), 4.0);
        assert_eq!(head[0], pos2(100.0, 0.0));
        assert!((base.x - (100.0 - 22.0)).abs() < 1e-4 && base.y == 0.0);
        assert!(head[1].y > 0.0 && head[2].y < 0.0);
        // 太短时箭头不超过箭杆。
        let (_, base) = arrow_head(pos2(0.0, 0.0), pos2(5.0, 0.0), 4.0);
        assert_eq!(base, pos2(0.0, 0.0));
    }

    #[test]
    fn undo_redo_and_erase() {
        let mut marks = Marks::default();
        assert!(!marks.can_undo());
        marks.push(stroke(&[(0.0, 0.0), (10.0, 0.0)], 2.0));
        marks.push(stroke(&[(0.0, 50.0), (10.0, 50.0)], 2.0));
        assert_eq!(marks.items().len(), 2);
        assert!(marks.undo());
        assert_eq!(marks.items().len(), 1);
        assert!(marks.redo());
        assert_eq!(marks.items().len(), 2);
        assert!(!marks.redo());

        assert!(!marks.erase_at(pos2(5.0, 25.0), 2.0));
        assert!(marks.erase_at(pos2(5.0, 51.0), 2.0));
        assert_eq!(marks.items().len(), 1);
        assert!(marks.undo());
        assert_eq!(marks.items().len(), 2);
        // 新的修改让重做失效。
        marks.push(stroke(&[(0.0, 80.0)], 2.0));
        assert!(!marks.can_redo());
        assert!(distance_to_polyline(pos2(3.0, 4.0), &[pos2(0.0, 0.0)]) == 5.0);
    }
}
