//! 浮在内容上面的栏（面板的顶部栏、底部栏，设置页的标题栏）的玻璃背景，参照 PasteNow。
//!
//! 内容一直延伸到栏的下面，但栏所在的地方不直接画（内容的裁剪区域避开栏）：
//! - 真正的窗口里由 `liquid_glass` 的着色器把下面的内容画成渐进的模糊；
//! - 没有 wgpu 渲染器时（离屏测试）用下面的办法在 egui 里模拟。
//!
//! 模拟时 egui 不能模糊背景：列表滚到顶部栏下面的部分不直接画（列表的裁剪区域从栏的下边缘开始），
//! 而是把这些卡片的图形（列表在图层里画下的那一段）读回来，在顶部栏的图层里按圆盘上的采样点偏移画多份，
//! 每份用同样低的不透明度叠起来，近似一次模糊。最后再盖一层半透明底色。

use std::f32::consts::TAU;
use std::ops::Range;

use egui::epaint::{shape_transform, Mesh};
use egui::layers::ShapeIdx;
use egui::{Color32, Context, LayerId, Painter, Rect, Shape, Stroke, Vec2};

use super::liquid_glass::LiquidGlass;
use super::style;
use crate::platform::panel as native_panel;

/// 栏的下边缘（底部栏是上边缘）渐隐到内容里的高度。
const EDGE_FADE: f32 = 10.0;

/// 栏贴着窗口的哪一边。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

/// 栏的背景：先占位，栏的内容排完、知道大小后再画在内容下面。
pub struct Bar {
    corners: ShapeIdx,
    corners_tint: ShapeIdx,
    background: ShapeIdx,
    fade: ShapeIdx,
}

impl Bar {
    pub fn begin(painter: &Painter) -> Self {
        Self {
            corners: painter.add(Shape::Noop),
            corners_tint: painter.add(Shape::Noop),
            background: painter.add(Shape::Noop),
            fade: painter.add(Shape::Noop),
        }
    }

    /// `rect` 是栏的范围（贴着窗口的 `edge` 边），`source` 是栏下面的内容，`tint` 是压在内容上的底色。
    /// `liquid` 是液态玻璃与这个栏在这一帧里的序号；没有时在 egui 里模拟。
    pub fn finish(
        self,
        painter: &Painter,
        rect: Rect,
        edge: Edge,
        source: Option<&Source>,
        tint: Color32,
        liquid: Option<(&LiquidGlass, usize)>,
    ) {
        let radius = native_panel::CORNER_RADIUS as f32;
        if let Some((liquid, slot)) = liquid {
            let shape = liquid.shape(painter.ctx(), slot, rect, edge, radius, tint);
            painter.with_clip_rect(rect).set(self.background, shape);
            return;
        }
        let blurred = source
            .map(|source| blurred(painter.ctx(), source, rect))
            .unwrap_or_default();
        let covered = !blurred.is_empty();
        // 窗口的四个角是圆的，角外透明：贴边的一条（圆角的高度）只在两个圆角之间画模糊的内容。
        let (band, inner, tint_rect, fade) = match edge {
            Edge::Top => (
                rect.with_max_y(rect.top() + radius),
                rect.with_min_y(rect.top() + radius),
                rect.with_max_y(rect.bottom() + radius * 2.0),
                Rect::from_min_size(rect.left_bottom(), Vec2::new(rect.width(), EDGE_FADE)),
            ),
            Edge::Bottom => (
                rect.with_min_y(rect.bottom() - radius),
                rect.with_max_y(rect.bottom() - radius),
                rect.with_min_y(rect.top() - radius * 2.0),
                Rect::from_min_max(
                    rect.left_top() - Vec2::new(0.0, EDGE_FADE),
                    rect.right_top(),
                ),
            ),
        };
        painter
            .with_clip_rect(band.shrink2(Vec2::new(radius, 0.0)))
            .set(self.corners, Shape::Vec(blurred.clone()));
        // 底色只要贴边的两个角是圆的：画一个更大的圆角矩形，裁掉朝内容的一半。
        let tint_shape = style::continuous_rect(tint_rect, radius, tint, Stroke::NONE);
        painter
            .with_clip_rect(band)
            .set(self.corners_tint, tint_shape.clone());
        let mut shapes = blurred;
        shapes.push(tint_shape);
        painter
            .with_clip_rect(inner)
            .set(self.background, Shape::Vec(shapes));
        if covered {
            // 有内容滚到栏下面时，栏的边缘渐隐到内容里，不是一条硬边。
            let (from, to) = match edge {
                Edge::Top => (tint, Color32::TRANSPARENT),
                Edge::Bottom => (Color32::TRANSPARENT, tint),
            };
            painter.set(self.fade, vertical_gradient(fade, from, to));
        }
    }
}

/// 模糊半径（点）：PasteNow 的玻璃下面还能认出文字的颜色和大致轮廓。
const RADIUS: f32 = 5.0;

/// 列表画下的图形：所在图层与在图层里的序号范围。
#[derive(Clone, Debug)]
pub struct Source {
    pub layer: LayerId,
    pub shapes: Range<usize>,
}

impl Source {
    /// 图层里目前已有的图形数，开始、结束画列表时各取一次。
    pub fn shape_count(ctx: &Context, layer: LayerId) -> usize {
        ctx.graphics(|graphics| {
            graphics
                .get(layer)
                .map_or(0, |list| list.all_entries().len())
        })
    }
}

/// 列表里落在 `rect` 附近的图形模糊后的样子。返回的图形坐标不变，调用方把它们裁剪到 `rect` 里。
pub fn blurred(ctx: &Context, source: &Source, rect: Rect) -> Vec<Shape> {
    let near = rect.expand(RADIUS);
    let sources: Vec<Shape> = ctx.graphics(|graphics| {
        graphics
            .get(source.layer)
            .map(|list| {
                list.all_entries()
                    .skip(source.shapes.start)
                    .take(source.shapes.len())
                    .filter(|clipped| clipped.shape.visual_bounding_rect().intersects(near))
                    .map(|clipped| clipped.shape.clone())
                    .collect()
            })
            .unwrap_or_default()
    });
    if sources.is_empty() {
        return Vec::new();
    }
    let offsets = offsets();
    // 细的文字笔画只被少数几份盖到：每份的不透明度相同，叠出来才是平均（模糊）而不是清晰的芯。
    // 总量取两倍，大块的颜色（图标、卡片底色）叠完接近原来的浓度，文字也不至于淡得看不见。
    let opacity = (2.0 / offsets.len() as f32).min(1.0);
    let mut shapes = Vec::with_capacity(sources.len() * offsets.len());
    for offset in offsets {
        for source in &sources {
            let mut shape = source.clone();
            shape.translate(offset);
            fade(&mut shape, opacity);
            shapes.push(shape);
        }
    }
    shapes
}

/// 采样点：圆心一个，外面三圈各 6、12、18 个。相邻采样点相距不到 2 点，
/// 细的文字笔画叠出来是一片柔和的颜色，而不是几份错开的重影。
fn offsets() -> Vec<Vec2> {
    let mut offsets = vec![Vec2::ZERO];
    for (ring, count) in [(1.0, 6), (2.0, 12), (3.0, 18)] {
        let radius = RADIUS * ring / 3.0;
        let phase = ring * 0.37;
        for i in 0..count {
            let angle = phase + i as f32 / count as f32 * TAU;
            offsets.push(Vec2::angled(angle) * radius);
        }
    }
    offsets
}

fn fade(shape: &mut Shape, opacity: f32) {
    match shape {
        // 文字有自己的不透明度，不必复制字形网格去改顶点颜色。
        Shape::Text(text) => text.opacity_factor *= opacity,
        Shape::Vec(shapes) => shapes.iter_mut().for_each(|shape| fade(shape, opacity)),
        _ => shape_transform::adjust_colors(shape, move |color| {
            *color = color.gamma_multiply(opacity);
        }),
    }
}

/// 上下渐变的矩形。
pub fn vertical_gradient(rect: Rect, top: Color32, bottom: Color32) -> Shape {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(2, 1, 3);
    Shape::mesh(mesh)
}
