//! 矢量图标：直接用画笔绘制，不依赖字体里的字形，因此总能精确居中。
//!
//! 所有图标都在 16×16 的设计网格里定义，绘制时缩放到目标矩形。

use std::f32::consts::{FRAC_PI_2, TAU};

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

/// 界面里用到的矢量图标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Search,
    Gear,
    Pin,
    Plus,
    Back,
    Pause,
    Play,
    Keyboard,
    Star,
    StarFilled,
    Close,
    Check,
    Warning,
    Document,
    Clock,
    Trash,
    Link,
    Image,
    Folder,
    Box,
    Pencil,
    Return,
    Tab,
    List,
    ChevronUp,
    ChevronDown,
    // 截屏标注工具栏（参照 Snipaste 的工具栏）。
    /// 矩形 / 椭圆工具。
    Shapes,
    Rectangle,
    Ellipse,
    Arrow,
    Marker,
    Mosaic,
    Text,
    Eraser,
    Undo,
    Redo,
    Save,
    Copy,
    /// 长截图（滚动截图）。
    ScrollCapture,
    /// 提取文字（OCR）。
    Ocr,
    /// 放大。
    ZoomIn,
    /// 缩小。
    ZoomOut,
    /// 排序（三条由长到短的横线）。
    Sort,
}

/// 16×16 网格坐标到屏幕坐标的映射。
struct Grid {
    rect: Rect,
    scale: f32,
}

impl Grid {
    /// 取 `rect` 中居中的正方形作为 16 × 16 的网格。
    fn new(rect: Rect) -> Self {
        let side = rect.width().min(rect.height());
        let square = Rect::from_center_size(rect.center(), Vec2::splat(side));
        Self {
            rect: square,
            scale: side / 16.0,
        }
    }

    /// 网格坐标转成屏幕坐标。
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.rect.min + Vec2::new(x, y) * self.scale
    }

    /// 网格中的长度转成屏幕长度。
    fn r(&self, value: f32) -> f32 {
        value * self.scale
    }

    /// 网格坐标表示的矩形。
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(self.p(x0, y0), self.p(x1, y1))
    }
}

/// 在 `rect` 中画图标（按 16 × 16 的网格设计，缩放到 `rect` 的短边）。
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
            line(&[(10.4, 10.4), (13.6, 13.6)]);
        }
        Icon::Gear => {
            let center = g.p(8.0, 8.0);
            let teeth = 8;
            let mut points = Vec::with_capacity(teeth * 4);
            for i in 0..teeth {
                let a = i as f32 / teeth as f32 * TAU;
                let step = TAU / teeth as f32;
                for (offset, radius) in [(-0.16, 5.0), (-0.09, 6.6), (0.09, 6.6), (0.16, 5.0)] {
                    let angle = a + offset * step * 2.2;
                    points.push(center + Vec2::angled(angle) * g.r(radius));
                }
            }
            painter.add(Shape::closed_line(points, stroke));
            painter.circle_stroke(center, g.r(2.0), stroke);
        }
        Icon::Pin => {
            // 图钉：帽、钉身、底座、钉针，整体右倾 45°。
            let rotate = |x: f32, y: f32| -> (f32, f32) {
                let (s, c) = (std::f32::consts::FRAC_PI_4).sin_cos();
                let (dx, dy) = (x - 8.0, y - 8.0);
                (8.0 + dx * c - dy * s, 8.0 + dx * s + dy * c)
            };
            let shape = |points: &[(f32, f32)]| -> Vec<(f32, f32)> {
                points.iter().map(|&(x, y)| rotate(x, y)).collect()
            };
            closed(
                &shape(&[(6.0, 1.5), (10.0, 1.5), (9.6, 3.0), (6.4, 3.0)]),
                true,
            );
            closed(
                &shape(&[(6.6, 3.0), (9.4, 3.0), (10.4, 7.6), (5.6, 7.6)]),
                false,
            );
            line(&shape(&[(3.6, 8.2), (12.4, 8.2)]));
            line(&shape(&[(8.0, 8.4), (8.0, 14.6)]));
        }
        Icon::Plus => {
            line(&[(8.0, 3.0), (8.0, 13.0)]);
            line(&[(3.0, 8.0), (13.0, 8.0)]);
        }
        Icon::Back => line(&[(10.0, 3.0), (5.0, 8.0), (10.0, 13.0)]),
        Icon::Pause => {
            painter.rect_filled(g.rect(4.2, 3.0, 6.8, 13.0), g.r(1.0), color);
            painter.rect_filled(g.rect(9.2, 3.0, 11.8, 13.0), g.r(1.0), color);
        }
        Icon::Play => closed(&[(4.8, 2.8), (13.0, 8.0), (4.8, 13.2)], true),
        Icon::Keyboard => {
            painter.rect_stroke(
                g.rect(1.5, 4.0, 14.5, 12.5),
                g.r(1.6),
                stroke,
                egui::StrokeKind::Middle,
            );
            for row in [6.4, 8.4] {
                for x in [4.0, 6.4, 8.8, 11.2] {
                    painter.rect_filled(g.rect(x - 0.6, row - 0.6, x + 0.6, row + 0.6), 0.0, color);
                }
            }
            line(&[(5.2, 10.5), (10.8, 10.5)]);
        }
        Icon::Star | Icon::StarFilled => {
            let center = g.p(8.0, 8.4);
            let points: Vec<Pos2> = (0..10)
                .map(|i| {
                    let radius = if i % 2 == 0 { g.r(6.8) } else { g.r(2.9) };
                    let angle = -FRAC_PI_2 + i as f32 * TAU / 10.0;
                    center + Vec2::angled(angle) * radius
                })
                .collect();
            if icon == Icon::StarFilled {
                // 五角星不是凸多边形：用中心点和每条边组成的三角形填充。
                for i in 0..points.len() {
                    let next = points[(i + 1) % points.len()];
                    painter.add(Shape::convex_polygon(
                        vec![center, points[i], next],
                        color,
                        Stroke::NONE,
                    ));
                }
            } else {
                painter.add(Shape::closed_line(points, stroke));
            }
        }
        Icon::Close => {
            line(&[(4.5, 4.5), (11.5, 11.5)]);
            line(&[(11.5, 4.5), (4.5, 11.5)]);
        }
        Icon::Check => line(&[(3.2, 8.4), (6.6, 11.6), (12.8, 4.6)]),
        Icon::Warning => {
            closed(&[(8.0, 2.0), (14.6, 13.6), (1.4, 13.6)], false);
            line(&[(8.0, 6.4), (8.0, 9.6)]);
            painter.circle_filled(g.p(8.0, 11.6), g.r(0.85), color);
        }
        Icon::Document => {
            closed(
                &[
                    (3.6, 1.8),
                    (9.8, 1.8),
                    (12.6, 4.6),
                    (12.6, 14.2),
                    (3.6, 14.2),
                ],
                false,
            );
            line(&[(9.8, 1.8), (9.8, 4.6), (12.6, 4.6)]);
            line(&[(5.8, 7.6), (10.4, 7.6)]);
            line(&[(5.8, 9.9), (10.4, 9.9)]);
            line(&[(5.8, 12.0), (8.6, 12.0)]);
        }
        Icon::Clock => {
            painter.circle_stroke(g.p(8.0, 8.0), g.r(6.2), stroke);
            line(&[(8.0, 8.0), (8.0, 4.4)]);
            line(&[(8.0, 8.0), (10.8, 9.6)]);
        }
        Icon::Trash => {
            line(&[(2.8, 4.6), (13.2, 4.6)]);
            line(&[(6.4, 4.4), (6.4, 2.6), (9.6, 2.6), (9.6, 4.4)]);
            closed(
                &[(4.4, 4.8), (11.6, 4.8), (10.8, 14.0), (5.2, 14.0)],
                false,
            );
        }
        Icon::Link => {
            // 两个相扣的圆角环。
            let link = |cx: f32, cy: f32| {
                let points: Vec<(f32, f32)> = (0..24)
                    .map(|i| {
                        let t = i as f32 / 24.0 * TAU;
                        let (s, c) = t.sin_cos();
                        // 拉长的椭圆，再旋转 -45°。
                        let (x, y) = (c * 3.6, s * 2.1);
                        let (rs, rc) = (-std::f32::consts::FRAC_PI_4).sin_cos();
                        (cx + x * rc - y * rs, cy + x * rs + y * rc)
                    })
                    .collect();
                closed(&points, false);
            };
            link(5.9, 10.1);
            link(10.1, 5.9);
        }
        Icon::Image => {
            painter.rect_stroke(
                g.rect(1.8, 3.0, 14.2, 13.0),
                g.r(1.6),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.circle_filled(g.p(5.4, 6.4), g.r(1.25), color);
            line(&[
                (2.4, 12.0),
                (6.4, 8.2),
                (9.0, 10.6),
                (11.0, 8.6),
                (13.6, 11.4),
            ]);
        }
        Icon::Folder => closed(
            &[
                (1.8, 3.6),
                (6.2, 3.6),
                (7.8, 5.4),
                (14.2, 5.4),
                (14.2, 13.2),
                (1.8, 13.2),
            ],
            false,
        ),
        Icon::Box => {
            closed(&[(2.4, 5.2), (13.6, 5.2), (13.6, 13.6), (2.4, 13.6)], false);
            closed(&[(2.4, 5.2), (4.2, 2.4), (11.8, 2.4), (13.6, 5.2)], false);
            line(&[(6.4, 8.0), (9.6, 8.0)]);
        }
        Icon::Return => {
            line(&[(12.8, 3.4), (12.8, 9.4), (3.6, 9.4)]);
            line(&[(6.4, 6.6), (3.6, 9.4), (6.4, 12.2)]);
        }
        Icon::Tab => {
            line(&[(2.6, 8.0), (11.6, 8.0)]);
            line(&[(8.8, 5.2), (11.6, 8.0), (8.8, 10.8)]);
            line(&[(13.4, 4.4), (13.4, 11.6)]);
        }
        Icon::Pencil => {
            closed(
                &[
                    (10.6, 2.6),
                    (13.4, 5.4),
                    (5.6, 13.2),
                    (2.4, 13.6),
                    (2.8, 10.4),
                ],
                false,
            );
            line(&[(9.2, 4.0), (12.0, 6.8)]);
        }
        Icon::List => {
            for y in [4.0, 8.0, 12.0] {
                painter.circle_filled(g.p(2.9, y), g.r(1.0), color);
                line(&[(5.8, y), (13.8, y)]);
            }
        }
        Icon::ChevronUp => line(&[(3.0, 10.5), (8.0, 5.5), (13.0, 10.5)]),
        Icon::ChevronDown => line(&[(3.0, 5.5), (8.0, 10.5), (13.0, 5.5)]),
        Icon::Shapes => {
            painter.rect_stroke(
                g.rect(2.4, 2.8, 10.4, 10.0),
                g.r(1.6),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.circle_stroke(g.p(10.6, 10.6), g.r(3.4), stroke);
        }
        Icon::Rectangle => {
            painter.rect_stroke(
                g.rect(2.6, 3.6, 13.4, 12.4),
                g.r(1.2),
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Ellipse => {
            painter.add(Shape::ellipse_stroke(
                g.p(8.0, 8.0),
                Vec2::new(g.r(5.6), g.r(4.6)),
                stroke,
            ));
        }
        Icon::Arrow => {
            line(&[(3.2, 12.8), (12.2, 3.8)]);
            line(&[(6.6, 3.6), (12.4, 3.6), (12.4, 9.4)]);
        }
        Icon::Marker => {
            // 斜放的记号笔：笔身、扁平的笔尖，下面一道划痕。
            closed(&[(9.8, 2.2), (13.4, 5.8), (8.2, 11.0), (4.6, 7.4)], false);
            closed(&[(4.6, 7.4), (8.2, 11.0), (5.8, 12.6), (3.0, 9.8)], true);
            line(&[(2.2, 14.2), (13.8, 14.2)]);
        }
        Icon::Mosaic => {
            let cell = 11.0 / 3.0;
            for row in 0..3 {
                for col in 0..3 {
                    if (row + col) % 2 == 0 {
                        let (x, y) = (2.5 + col as f32 * cell, 2.5 + row as f32 * cell);
                        painter.rect_filled(g.rect(x, y, x + cell, y + cell), 0.0, color);
                    }
                }
            }
            painter.rect_stroke(
                g.rect(2.5, 2.5, 13.5, 13.5),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Text => {
            line(&[(3.4, 5.0), (3.4, 3.2), (12.6, 3.2), (12.6, 5.0)]);
            line(&[(8.0, 3.2), (8.0, 13.0)]);
            line(&[(5.8, 13.0), (10.2, 13.0)]);
        }
        Icon::Eraser => {
            closed(&[(9.2, 2.6), (13.6, 7.0), (7.6, 13.0), (3.2, 8.6)], false);
            line(&[(6.2, 5.6), (10.6, 10.0)]);
            line(&[(7.6, 13.0), (13.8, 13.0)]);
        }
        Icon::Undo | Icon::Redo => {
            // ↶：上面一道向左的箭头，右侧绕一个半圆回到下方；重做左右镜像。
            let mirror = icon == Icon::Redo;
            let x = |x: f32| if mirror { 16.0 - x } else { x };
            let mut points = vec![g.p(x(3.6), 6.0)];
            let center = (9.6, 9.0);
            for i in 0..=12 {
                let angle = -FRAC_PI_2 + i as f32 / 12.0 * std::f32::consts::PI;
                let (sin, cos) = angle.sin_cos();
                points.push(g.p(x(center.0 + 3.0 * cos), center.1 + 3.0 * sin));
            }
            points.push(g.p(x(6.0), 12.0));
            painter.add(Shape::line(points, stroke));
            line(&[(x(6.0), 3.6), (x(3.6), 6.0), (x(6.0), 8.4)]);
        }
        Icon::Save => {
            closed(
                &[
                    (2.6, 2.6),
                    (11.0, 2.6),
                    (13.4, 5.0),
                    (13.4, 13.4),
                    (2.6, 13.4),
                ],
                false,
            );
            line(&[(5.2, 2.6), (5.2, 6.0), (10.0, 6.0), (10.0, 2.6)]);
            closed(&[(4.6, 13.4), (4.6, 9.2), (11.4, 9.2), (11.4, 13.4)], false);
        }
        Icon::ScrollCapture => {
            // 一页内容，下面一个向下的箭头。
            painter.rect_stroke(
                g.rect(3.0, 2.6, 13.0, 9.0),
                g.r(1.2),
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[(8.0, 5.0), (8.0, 13.6)]);
            line(&[(5.4, 11.0), (8.0, 13.6), (10.6, 11.0)]);
        }
        Icon::Ocr => {
            // 取景框的四个角，中间一个“T”。
            for (x, y, dx, dy) in [
                (2.6, 2.6, 1.0, 1.0),
                (13.4, 2.6, -1.0, 1.0),
                (2.6, 13.4, 1.0, -1.0),
                (13.4, 13.4, -1.0, -1.0),
            ] {
                line(&[(x, y + 3.0 * dy), (x, y), (x + 3.0 * dx, y)]);
            }
            line(&[(5.4, 5.6), (10.6, 5.6)]);
            line(&[(8.0, 5.6), (8.0, 10.8)]);
        }
        Icon::ZoomIn | Icon::ZoomOut => {
            // 放大镜，镜片里一个加号（放大）或减号（缩小）。
            painter.circle_stroke(g.p(7.0, 7.0), g.r(4.6), stroke);
            line(&[(10.4, 10.4), (13.6, 13.6)]);
            line(&[(4.8, 7.0), (9.2, 7.0)]);
            if icon == Icon::ZoomIn {
                line(&[(7.0, 4.8), (7.0, 9.2)]);
            }
        }
        Icon::Sort => {
            for (y, right) in [(4.6, 13.2), (8.0, 10.4), (11.4, 7.6)] {
                line(&[(2.8, y), (right, y)]);
            }
        }
        Icon::Copy => {
            painter.rect_stroke(
                g.rect(2.6, 5.4, 10.6, 13.4),
                g.r(1.4),
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[
                (5.4, 5.4),
                (5.4, 3.8),
                (6.2, 2.6),
                (12.2, 2.6),
                (13.4, 3.8),
                (13.4, 9.8),
                (12.2, 10.6),
                (10.6, 10.6),
            ]);
        }
    }
}

/// 在 `rect` 中央绘制 `size` 大小的图标。
pub fn paint_centered(painter: &Painter, rect: Rect, size: f32, icon: Icon, color: Color32) {
    paint(
        painter,
        Rect::from_center_size(rect.center(), Vec2::splat(size)),
        icon,
        color,
    );
}
