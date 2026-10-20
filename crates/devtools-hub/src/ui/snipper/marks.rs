//! 在截屏遮罩上绘制标注（像素坐标、线宽换算成点），以及正在输入的文字。

use egui::{
    Color32, FontId, Frame, Id, Margin, Order, Painter, Pos2, Rect, Shape, Stroke, StrokeKind,
};

use super::View;
use crate::snip::annotation::{arrow_head, Mark};

/// 画一个标注。
pub(super) fn paint(painter: &Painter, view: &View, mark: &Mark) {
    let point = |p: Pos2| view.to_point(p);
    let length = |px: f32| px / view.scale;
    match mark {
        Mark::Shape {
            rect,
            ellipse,
            color,
            width,
        } => {
            let rect = view.to_points(*rect);
            let stroke = Stroke::new(length(*width), *color);
            if *ellipse {
                painter.add(Shape::ellipse_stroke(
                    rect.center(),
                    rect.size() / 2.0,
                    stroke,
                ));
            } else {
                painter.rect_stroke(rect, 0.0, stroke, StrokeKind::Middle);
            }
        }
        Mark::Arrow {
            from,
            to,
            color,
            width,
        } => {
            let (head, base) = arrow_head(*from, *to, *width);
            painter.line_segment(
                [point(*from), point(base)],
                Stroke::new(length(*width), *color),
            );
            painter.add(Shape::convex_polygon(
                head.iter().map(|p| point(*p)).collect(),
                *color,
                Stroke::NONE,
            ));
        }
        Mark::Stroke {
            points,
            color,
            width,
        } => {
            let points: Vec<Pos2> = points.iter().map(|p| point(*p)).collect();
            let width = length(*width);
            if let [only] = points.as_slice() {
                painter.circle_filled(*only, width / 2.0, *color);
            } else {
                painter.add(Shape::line(points.clone(), Stroke::new(width, *color)));
                // 不透明的笔迹两端补成圆头；半透明的记号笔补了会在两端叠出深色的圆。
                if color.is_opaque() {
                    for end in [points[0], points[points.len() - 1]] {
                        painter.circle_filled(end, width / 2.0, *color);
                    }
                }
            }
        }
        Mark::Mosaic(mosaic) => {
            // 相邻的格子各向外扩半个物理像素，避免抗锯齿在格子之间留下细缝。
            let overlap = 0.5 / view.scale;
            for (cell, color) in &mosaic.cells {
                painter.rect_filled(view.to_points(*cell).expand(overlap), 0.0, *color);
            }
        }
        Mark::Text {
            pos,
            text,
            color,
            size,
            wrap,
            ..
        } => {
            // 与输入时一样在选区右边缘自动换行。
            let galley = painter.layout(
                text.clone(),
                FontId::proportional(length(*size)),
                *color,
                length(*wrap),
            );
            painter.galley(point(*pos), galley, *color);
        }
    }
}

/// 正在输入的文字（像素坐标）。
pub(super) struct TextDraft {
    /// 左上角。
    pub pos: Pos2,
    pub text: String,
    pub color: Color32,
    /// 字号（像素）。
    pub size: f32,
    /// 是否已经让输入框获得焦点（只在出现时请求一次）。
    focused: bool,
    /// 上一帧输入框的范围（点），鼠标点在里面时交给输入框处理。
    pub rect: Option<Rect>,
}

impl TextDraft {
    pub fn new(pos: Pos2, color: Color32, size: f32) -> Self {
        Self {
            pos,
            text: String::new(),
            color,
            size,
            focused: false,
            rect: None,
        }
    }

    /// 换行宽度（点）：从输入框左边到选区（`area`，点）右边缘，至少两个字宽。
    fn wrap_width(&self, view: &View, area: Rect) -> f32 {
        let font = self.size / view.scale;
        (area.right() - view.to_point(self.pos).x - 2.0).max(font * 2.0)
    }

    /// 显示输入框：浮在截图上、没有边框，外面画一圈虚线。`area` 是选区（点）。
    pub fn show(&mut self, ctx: &egui::Context, view: &View, area: Rect) {
        let font = FontId::proportional(self.size / view.scale);
        let painter = ctx.layer_painter(egui::LayerId::new(
            Order::Foreground,
            Id::new("snip-text-frame"),
        ));
        // 输入框先向右延伸，碰到选区右边缘才自动换行、向下延伸；回车照常换行。
        let pos = view.to_point(self.pos);
        let max_width = self.wrap_width(view, area);
        let sample = if self.text.is_empty() {
            " "
        } else {
            &self.text
        };
        let galley = painter.layout(sample.to_string(), font.clone(), self.color, max_width);
        // 留出一个字的位置给光标和输入法正在组字的内容。
        let width = (galley.size().x + font.size).min(max_width);
        let color = self.color;
        let layout_font = font.clone();
        let mut layouter = move |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap: f32| {
            let job = egui::text::LayoutJob::simple(
                text.as_str().to_owned(),
                layout_font.clone(),
                color,
                max_width,
            );
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let area = egui::Area::new(Id::new("snip-text"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .constrain(false)
            .show(ctx, |ui| {
                // Area 默认的可用宽度很窄，TextEdit 的宽度会被它截断而提前换行，这里放宽到选区右边缘。
                ui.set_max_width(max_width);
                let response = ui.add(
                    egui::TextEdit::multiline(&mut self.text)
                        .font(font)
                        .text_color(self.color)
                        .frame(Frame::NONE)
                        .background_color(Color32::TRANSPARENT)
                        .margin(Margin::ZERO)
                        .desired_width(width)
                        .desired_rows(1)
                        .clip_text(false)
                        .layouter(&mut layouter),
                );
                // 输入框是在一次点击里出现的：同一次点击的按下、松开都不在它上面，
                // egui 会认为点在了别处而让它失去焦点。等鼠标按键平静下来的那一帧再要焦点。
                let clicking = ui.input(|i| {
                    i.pointer.any_down() || i.pointer.any_pressed() || i.pointer.any_released()
                });
                if !self.focused && !clicking {
                    response.request_focus();
                    self.focused = true;
                }
            });
        if !self.focused {
            ctx.request_repaint();
        }
        let rect = area.response.rect.expand(3.0);
        self.rect = Some(rect);
        let stroke = Stroke::new(1.0, Color32::from_gray(140));
        let corners = [
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
            rect.left_top(),
        ];
        painter.extend(Shape::dashed_line(&corners, stroke, 4.0, 3.0));
    }

    /// 输入结束：有文字时返回对应的标注（量出排版后的大小，橡皮擦要用）。
    pub fn finish(self, painter: &Painter, view: &View, area: Rect) -> Option<Mark> {
        if self.text.trim().is_empty() {
            return None;
        }
        let wrap = self.wrap_width(view, area);
        let galley = painter.layout(
            self.text.clone(),
            FontId::proportional(self.size / view.scale),
            self.color,
            wrap,
        );
        Some(Mark::Text {
            pos: self.pos,
            text: self.text,
            color: self.color,
            size: self.size,
            wrap: wrap * view.scale,
            extent: galley.size() * view.scale,
        })
    }
}
