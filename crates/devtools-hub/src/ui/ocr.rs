//! 提取文字的结果页面（方案见 `docs/微信滚动截图与文字提取实现分析.md` 的 4.2 节“系统 OCR”）。
//!
//! 截屏工具栏的“提取文字”（⌘E，长截图也有）和贴图右键菜单的“提取文字”把图片交到这里：
//! 后台线程调用系统的文字识别（`platform::ocr`），主窗口切到这个页面。左边是图片，识别出的每一行画上框，
//! 鼠标悬停时高亮，点击复制这一行；右边是合并好的文字，可以修改、选中后 ⌘C 复制；“复制全部”复制右边的文字。
//! 复制的内容照常记进剪贴板历史。

use std::sync::mpsc::{self, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::{
    Align, Color32, CursorIcon, Frame, Key, Layout, Margin, Modifiers, Rect, RichText, Sense,
    Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2,
};
use image::RgbaImage;

use super::fonts;
use super::icons::{self, Icon};
use super::style::{self, Palette};
use super::widgets::{centered_row, drag_window, icon_button};
use super::HubApp;
use crate::keys;
use crate::platform::{self, panel as native_panel, Anchor, Placement};
use crate::shared::{AppEvent, Shared};
use crate::snip::{self, ocr::Line};

/// 右边文字的字号。
const FONT_SIZE: f32 = 14.0;

/// 提取文字页面的状态：最近一次识别的图片和结果（窗口收起后保留）。
#[derive(Default)]
pub struct OcrView {
    job: Option<Job>,
    /// 固定窗口：失去焦点时不自动隐藏。
    pub pinned: bool,
}

/// 一次识别。
struct Job {
    image: Arc<RgbaImage>,
    texture: Option<TextureHandle>,
    state: State,
    /// 右边可以修改的文字（识别完成时填入）。
    text: String,
}

enum State {
    Running {
        receiver: mpsc::Receiver<Result<Vec<Line>>>,
        started: Instant,
    },
    Done {
        lines: Vec<Line>,
        elapsed: Duration,
    },
    Failed(String),
}

impl OcrView {
    /// 在后台线程识别 `image`（之前没识别完的结果不再要）。完成后发 [`AppEvent::OcrDone`] 唤醒界面。
    fn start(&mut self, image: Arc<RgbaImage>, shared: Arc<Shared>) {
        let (sender, receiver) = mpsc::channel();
        let input = image.clone();
        std::thread::Builder::new()
            .name("ocr".into())
            .spawn(move || {
                let started = Instant::now();
                let result = platform::ocr::recognize(&input);
                match &result {
                    Ok(lines) => tracing::info!(
                        "提取文字：{}×{}，{} 行，用时 {:?}",
                        input.width(),
                        input.height(),
                        lines.len(),
                        started.elapsed()
                    ),
                    Err(err) => tracing::warn!("提取文字失败：{err:#}"),
                }
                let _ = sender.send(result);
                shared.post(AppEvent::OcrDone);
            })
            .expect("创建文字识别线程失败");
        self.job = Some(Job {
            image,
            texture: None,
            state: State::Running {
                receiver,
                started: Instant::now(),
            },
            text: String::new(),
        });
    }

    /// 取回后台识别的结果（还没完成时什么都不做）。
    pub fn poll(&mut self) {
        let Some(job) = &mut self.job else {
            return;
        };
        let State::Running { receiver, started } = &job.state else {
            return;
        };
        let elapsed = started.elapsed();
        job.state = match receiver.try_recv() {
            Ok(Ok(lines)) => {
                job.text = snip::ocr::to_text(&lines);
                State::Done { lines, elapsed }
            }
            Ok(Err(err)) => State::Failed(format!("{err:#}")),
            Err(TryRecvError::Disconnected) => State::Failed("识别意外中断".into()),
            Err(TryRecvError::Empty) => return,
        };
    }

    /// 是否还在识别。
    fn running(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|job| matches!(job.state, State::Running { .. }))
    }
}

/// 页面上产生的操作，绘制结束后统一执行。
enum OcrAction {
    /// 复制一段文字（点击图片上的一行）。
    CopyLine(String),
    /// 复制右边的全部文字。
    CopyAll,
    Close,
}

impl HubApp {
    /// 提取图片里的文字：开始后台识别，主窗口切到提取文字页面（出现在屏幕中央）。
    pub(super) fn open_ocr(&mut self, image: Arc<RgbaImage>) {
        self.startup_hide_pending = false;
        self.cancel_hotkey_recording();
        self.ocr.start(image, self.shared.clone());
        self.page = super::Page::Ocr;
        native_panel::show(
            Anchor::Placement(Placement::ScreenCenter),
            self.settings.ocr_size,
        );
        self.ctx.request_repaint();
    }

    /// 绘制页面：顶部标题栏，底部状态与按钮，中间左边是图片、右边是文字。
    pub(super) fn ocr_ui(&mut self, ui: &mut Ui) {
        self.ocr.poll();
        if self.ocr.running() {
            // 识别中显示转圈；完成时后台线程会唤醒界面。
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        let palette = self.palette(ui.ctx());
        let mut actions = Vec::new();
        ui.ctx().input_mut(|input| {
            if input.consume_key(Modifiers::NONE, Key::Escape) {
                actions.push(OcrAction::Close);
            }
            if input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::C) {
                actions.push(OcrAction::CopyAll);
            }
        });

        egui::Panel::top("ocr-header")
            .show_separator_line(false)
            .frame(Frame::NONE.inner_margin(Margin {
                left: style::PAD,
                right: style::PAD,
                top: style::PAD,
                bottom: 8,
            }))
            .show(ui, |ui| self.ocr_header(ui, &palette, &mut actions));
        egui::Panel::bottom("ocr-footer")
            .show_separator_line(false)
            .frame(Frame::NONE.inner_margin(Margin {
                left: style::PAD,
                right: style::PAD,
                top: 8,
                bottom: style::PAD,
            }))
            .show(ui, |ui| self.ocr_footer(ui, &palette, &mut actions));
        egui::CentralPanel::default()
            .frame(Frame::NONE)
            .show(ui, |ui| {
                let Some(job) = &mut self.ocr.job else {
                    return;
                };
                egui::Panel::left("ocr-image")
                    .resizable(true)
                    .default_size(ui.available_width() * 0.5)
                    .size_range(160.0..=ui.available_width() * 0.75)
                    .frame(
                        Frame::NONE
                            .fill(palette.field)
                            .inner_margin(Margin::same(8)),
                    )
                    .show(ui, |ui| image_view(ui, job, &palette, &mut actions));
                text_view(ui, job, &palette);
            });

        self.apply_ocr_actions(actions);
    }

    /// 标题栏：图标、标题，右侧是关闭与固定按钮；空白处可以拖动窗口。
    fn ocr_header(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<OcrAction>) {
        drag_window(ui, ui.max_rect());
        centered_row(ui, 28.0, |ui| {
            let (badge, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
            ui.painter()
                .rect_filled(badge, style::SMALL_RADIUS, palette.accent);
            icons::paint_centered(ui.painter(), badge, 14.0, Icon::Ocr, Color32::WHITE);
            ui.label(
                RichText::new("提取文字")
                    .size(14.0)
                    .family(egui::FontFamily::Name(fonts::SEMIBOLD.into()))
                    .color(palette.text),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if icon_button(ui, Icon::Close, false, palette)
                    .on_hover_text(format!("关闭  {}", keys::ESCAPE))
                    .clicked()
                {
                    actions.push(OcrAction::Close);
                }
                let pinned = self.ocr.pinned;
                let pin = icon_button(ui, Icon::Pin, pinned, palette).on_hover_text(if pinned {
                    "已固定：点击其他应用时不隐藏"
                } else {
                    "固定窗口"
                });
                if pin.clicked() {
                    self.ocr.pinned = !pinned;
                }
            });
        });
    }

    /// 底栏：左边是识别状态，右边是“复制全部”。
    fn ocr_footer(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<OcrAction>) {
        centered_row(ui, 28.0, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().interact_size.y = 28.0;
                let has_text = self
                    .ocr
                    .job
                    .as_ref()
                    .is_some_and(|job| !job.text.trim().is_empty());
                if ui
                    .add_enabled(has_text, egui::Button::new("复制全部"))
                    .on_hover_text(format!(
                        "复制右边的文字  {}",
                        keys::shortcut(&[keys::SHIFT, keys::COMMAND, "C"])
                    ))
                    .clicked()
                {
                    actions.push(OcrAction::CopyAll);
                }
                ui.add_space(6.0);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    status_label(ui, self.ocr.job.as_ref(), palette);
                });
            });
        });
    }

    /// 执行页面上的操作。
    fn apply_ocr_actions(&mut self, actions: Vec<OcrAction>) {
        for action in actions {
            match action {
                OcrAction::CopyLine(text) => {
                    self.copy_ocr_text(&text);
                    self.show_toast("已复制这一行");
                }
                OcrAction::CopyAll => {
                    let Some(text) = self
                        .ocr
                        .job
                        .as_ref()
                        .map(|job| job.text.clone())
                        .filter(|text| !text.trim().is_empty())
                    else {
                        continue;
                    };
                    self.copy_ocr_text(&text);
                    self.show_toast("已复制");
                }
                OcrAction::Close => self.hide_panel(true),
            }
        }
    }

    /// 把文字写入剪贴板（不经 `Shared::write_pasteboard`，照常记进剪贴板历史）。
    fn copy_ocr_text(&self, text: &str) {
        let snapshot = devtools_clipboard::Snapshot::from_text(text);
        self.pasteboard().write(&snapshot);
    }
}

/// 底栏左边的状态：识别中转圈，完成后显示行数和用时，失败时显示原因。
fn status_label(ui: &mut Ui, job: Option<&Job>, palette: &Palette) {
    let (icon, color, text) = match job.map(|job| &job.state) {
        None => (None, palette.weak, "没有要识别的图片".to_owned()),
        Some(State::Running { .. }) => {
            ui.add(egui::Spinner::new().size(14.0).color(palette.weak));
            (None, palette.weak, "正在识别…".to_owned())
        }
        Some(State::Done { lines, .. }) if lines.is_empty() => (
            Some(Icon::Warning),
            palette.weak,
            "没有识别到文字".to_owned(),
        ),
        Some(State::Done { lines, elapsed }) => (
            Some(Icon::Check),
            palette.ok,
            format!(
                "识别出 {} 行 · 用时 {:.1} 秒 · 点击图片上的一行复制它",
                lines.len(),
                elapsed.as_secs_f32()
            ),
        ),
        Some(State::Failed(message)) => (Some(Icon::Warning), palette.warn, message.clone()),
    };
    if let Some(icon) = icon {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
        icons::paint(ui.painter(), rect, icon, color);
    }
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).truncate());
}

/// 左边的图片：缩小到栏宽（不放大），太高时上下滚动；识别出的每一行画上框，悬停高亮、点击复制。
fn image_view(ui: &mut Ui, job: &mut Job, palette: &Palette, actions: &mut Vec<OcrAction>) {
    let ctx = ui.ctx().clone();
    let texture = job
        .texture
        .get_or_insert_with(|| {
            let max = ctx.input(|i| i.max_texture_side);
            ctx.load_texture(
                "ocr-image",
                snip::texture_image(&job.image, max),
                TextureOptions::LINEAR,
            )
        })
        .clone();
    let pixels = Vec2::new(job.image.width() as f32, job.image.height() as f32);
    // 1 个图像像素对应 1 个物理像素，即 100%。
    let natural = pixels / ctx.pixels_per_point();
    egui::ScrollArea::vertical()
        .id_salt("ocr-image-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let fit = (ui.available_width() / natural.x).min(1.0);
            let size = natural * fit;
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            let painter = ui.painter_at(rect);
            if snip::has_alpha(&job.image) {
                painter.rect_filled(rect, 0.0, Color32::WHITE);
            }
            painter.image(
                texture.id(),
                rect,
                Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            let State::Done { lines, .. } = &job.state else {
                return;
            };
            let scale = size.x / pixels.x;
            let to_screen = |line: &Line| {
                Rect::from_min_max(
                    rect.min + line.rect.min.to_vec2() * scale,
                    rect.min + line.rect.max.to_vec2() * scale,
                )
                .expand(1.5)
            };
            let hovered = response
                .hover_pos()
                .and_then(|pos| lines.iter().find(|line| to_screen(line).contains(pos)));
            for line in lines {
                let area = to_screen(line);
                if hovered.is_some_and(|hovered| std::ptr::eq(hovered, line)) {
                    painter.rect_filled(area, 2.0, palette.accent.gamma_multiply(0.22));
                    painter.rect_stroke(
                        area,
                        2.0,
                        Stroke::new(1.5, palette.accent),
                        StrokeKind::Outside,
                    );
                } else {
                    painter.rect_stroke(
                        area,
                        2.0,
                        Stroke::new(1.0, palette.accent.gamma_multiply(0.55)),
                        StrokeKind::Outside,
                    );
                }
            }
            if let Some(line) = hovered {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
                let response = response.on_hover_text(&line.text);
                if response.clicked() {
                    actions.push(OcrAction::CopyLine(line.text.clone()));
                }
            }
        });
}

/// 右边的文字：可以修改、选中复制；识别中、没有结果时显示提示。
fn text_view(ui: &mut Ui, job: &mut Job, palette: &Palette) {
    let placeholder = match &job.state {
        State::Running { .. } => Some("正在识别…"),
        State::Done { lines, .. } if lines.is_empty() => Some("没有识别到文字"),
        State::Failed(_) => Some("识别失败"),
        State::Done { .. } => None,
    };
    if let Some(placeholder) = placeholder {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new(placeholder).size(13.0).color(palette.weak));
        });
        return;
    }
    Frame::NONE
        .inner_margin(Margin {
            left: 12,
            right: 4,
            top: 6,
            bottom: 0,
        })
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("ocr-text-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_sized(
                        ui.available_size(),
                        egui::TextEdit::multiline(&mut job.text)
                            .font(egui::FontId::proportional(FONT_SIZE))
                            .text_color(palette.text)
                            .frame(Frame::NONE)
                            .margin(Margin::ZERO),
                    );
                });
        });
}

#[cfg(test)]
impl OcrView {
    /// 测试用：不经系统识别，直接给出图片和识别结果；`lines` 为空时停在“正在识别”。
    pub(super) fn set_result(&mut self, image: RgbaImage, lines: Option<Vec<Line>>) {
        let (sender, receiver) = mpsc::channel();
        match lines {
            Some(lines) => sender.send(Ok(lines)).unwrap(),
            // 发送端一直不关闭，通道里也没有结果。
            None => std::mem::forget(sender),
        }
        self.job = Some(Job {
            image: Arc::new(image),
            texture: None,
            state: State::Running {
                receiver,
                started: Instant::now(),
            },
            text: String::new(),
        });
        self.poll();
    }

    /// 右边的文字。
    pub(super) fn text(&self) -> Option<&str> {
        self.job.as_ref().map(|job| job.text.as_str())
    }
}
