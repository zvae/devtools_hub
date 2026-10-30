//! 矢量图标：与宿主共用同一套设计语言（16×16 网格、单色描边），不依赖字体字形。
//!
//! 宿主 `ui/icons.rs` 的图标不能跨 crate 复用，这里按同样的网格与线宽重画搜索页需要的一小组。

use std::f32::consts::{FRAC_PI_4, TAU};

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};

/// 搜索页用到的图标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Search,
    Gear,
    Clock,
    File,
    Folder,
    Close,
    Back,
    ChevronDown,
    ChevronUp,
    Pin,
    Play,
    Pause,
    Plus,
    Trash,
    Warning,
    Check,
    Return,
}

/// 16×16 网格。
struct Grid {
    rect: Rect,
    scale: f32,
}

impl Grid {
    fn new(rect: Rect) -> Self {
        let side = rect.width().min(rect.height());
        Self {
            rect: Rect::from_center_size(rect.center(), Vec2::splat(side)),
            scale: side / 16.0,
        }
    }

    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.rect.min + Vec2::new(x, y) * self.scale
    }

    fn r(&self, value: f32) -> f32 {
        value * self.scale
    }

    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(self.p(x0, y0), self.p(x1, y1))
    }
}

/// 在 `rect` 中画图标（按 16×16 网格设计，缩放到 `rect` 的短边）。
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let g = Grid::new(rect);
    let stroke = Stroke::new(g.r(1.35).max(1.0), color);
    let line = |points: &[(f32, f32)]| {
        let points: Vec<Pos2> = points.iter().map(|&(x, y)| g.p(x, y)).collect();
        painter.add(Shape::line(points, stroke));
    };
    let closed = |points: &[(f32, f32)], fill: bool| {
        let points: Vec<Pos2> = points.iter().map(|&(x, y)| g.p(x, y)).collect();
        if fill {
            painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
        } else {
            painter.add(Shape::closed_line(points, stroke));
        }
    };
    match icon {
        Icon::Search => {
            painter.circle_stroke(g.p(7.0, 7.0), g.r(4.6), stroke);
            line(&[(10.4, 10.4), (13.4, 13.4)]);
        }
        Icon::Gear => {
            let center = g.p(8.0, 8.0);
            let teeth = 8;
            let mut points = Vec::with_capacity(teeth * 4);
            for i in 0..teeth {
                let a = i as f32 / teeth as f32 * TAU;
                let step = TAU / teeth as f32;
                for (offset, radius) in [(-0.16, 5.0), (-0.09, 6.6), (0.09, 6.6), (0.16, 5.0)] {
                    points.push(center + Vec2::angled(a + offset * step * 2.2) * g.r(radius));
                }
            }
            painter.add(Shape::closed_line(points, stroke));
            painter.circle_stroke(center, g.r(2.0), stroke);
        }
        Icon::Clock => {
            painter.circle_stroke(g.p(8.0, 8.0), g.r(6.2), stroke);
            line(&[(8.0, 8.0), (8.0, 4.4)]);
            line(&[(8.0, 8.0), (10.8, 9.6)]);
        }
        Icon::File => {
            closed(
                &[(4.0, 1.8), (9.6, 1.8), (12.6, 4.8), (12.6, 14.2), (4.0, 14.2)],
                false,
            );
            line(&[(9.6, 1.8), (9.6, 4.8), (12.6, 4.8)]);
        }
        Icon::Folder => {
            // 上沿的标签 + 圆角主体（填充，选中行里也更醒目）。
            painter.rect_filled(g.rect(1.8, 3.4, 7.0, 5.4), g.r(1.0), color.gamma_multiply(0.75));
            painter.rect_filled(g.rect(1.8, 4.6, 14.2, 13.2), g.r(2.0), color);
        }
        Icon::Close => {
            line(&[(4.6, 4.6), (11.4, 11.4)]);
            line(&[(11.4, 4.6), (4.6, 11.4)]);
        }
        Icon::Back => line(&[(10.0, 3.4), (5.4, 8.0), (10.0, 12.6)]),
        Icon::ChevronDown => line(&[(4.2, 6.2), (8.0, 10.0), (11.8, 6.2)]),
        Icon::ChevronUp => line(&[(4.2, 9.8), (8.0, 6.0), (11.8, 9.8)]),
        Icon::Pin => {
            let rotate = |x: f32, y: f32| -> (f32, f32) {
                let (s, c) = FRAC_PI_4.sin_cos();
                let (dx, dy) = (x - 8.0, y - 8.0);
                (8.0 + dx * c - dy * s, 8.0 + dx * s + dy * c)
            };
            let shape = |points: &[(f32, f32)]| -> Vec<(f32, f32)> {
                points.iter().map(|&(x, y)| rotate(x, y)).collect()
            };
            closed(&shape(&[(6.0, 1.6), (10.0, 1.6), (9.6, 3.0), (6.4, 3.0)]), true);
            closed(&shape(&[(6.6, 3.0), (9.4, 3.0), (10.4, 7.6), (5.6, 7.6)]), false);
            line(&shape(&[(3.6, 8.2), (12.4, 8.2)]));
            line(&shape(&[(8.0, 8.4), (8.0, 14.4)]));
        }
        Icon::Play => closed(&[(5.0, 3.0), (13.0, 8.0), (5.0, 13.0)], true),
        Icon::Pause => {
            painter.rect_filled(g.rect(4.2, 3.0, 6.8, 13.0), g.r(1.0), color);
            painter.rect_filled(g.rect(9.2, 3.0, 11.8, 13.0), g.r(1.0), color);
        }
        Icon::Plus => {
            line(&[(8.0, 3.4), (8.0, 12.6)]);
            line(&[(3.4, 8.0), (12.6, 8.0)]);
        }
        Icon::Trash => {
            line(&[(2.8, 4.6), (13.2, 4.6)]);
            line(&[(6.4, 4.4), (6.4, 2.6), (9.6, 2.6), (9.6, 4.4)]);
            closed(
                &[(4.4, 4.8), (11.6, 4.8), (10.8, 14.0), (5.2, 14.0)],
                false,
            );
        }
        Icon::Warning => {
            closed(&[(8.0, 2.2), (14.2, 13.4), (1.8, 13.4)], false);
            line(&[(8.0, 6.0), (8.0, 9.6)]);
            painter.circle_filled(g.p(8.0, 11.6), g.r(0.7), color);
        }
        Icon::Check => line(&[(3.4, 8.4), (6.6, 11.6), (12.6, 4.4)]),
        Icon::Return => {
            line(&[(12.2, 4.0), (12.2, 9.0), (5.0, 9.0)]);
            line(&[(7.4, 6.6), (5.0, 9.0), (7.4, 11.4)]);
        }
    }
    let _ = StrokeKind::Inside;
}

/// 在 `rect` 中居中画一个 `size` 见方的图标。
pub fn paint_centered(painter: &Painter, rect: Rect, size: f32, icon: Icon, color: Color32) {
    let square = Rect::from_center_size(rect.center(), Vec2::splat(size));
    paint(painter, square, icon, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_maps_design_coordinates() {
        let g = Grid::new(Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::splat(32.0)));
        // 16 网格缩放到 32px：中心点落在正中。
        assert_eq!(g.p(8.0, 8.0), egui::pos2(16.0, 16.0));
        assert_eq!(g.r(1.35), 2.7);
        let corner = g.rect(0.0, 0.0, 16.0, 16.0);
        assert_eq!(corner.width(), 32.0);
    }
}
