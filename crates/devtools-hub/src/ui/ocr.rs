//! 提取文字的结果页面（方案见 `docs/微信滚动截图与文字提取实现分析.md` 的 4.2 节“系统 OCR”）。
//!
//! 截屏工具栏的“提取文字”（⌘E，长截图也有）和贴图右键菜单的“提取文字”把图片交到这里：
//! 后台线程调用系统的文字识别（`platform::ocr`），主窗口切到这个页面。左边是图片（识别出的文字不画框），
//! 右边是合并好的文字，可以修改、选中后 ⌘C 复制。
//!
//! 图片上的文字可以用鼠标拖动选中（精确到字，也可以跨行）：右边对应的文字同步标亮，并滚到看得见的地方。
//! 单击选中整行；改动右边的文字之后，识别出的位置和文字就对不上了，标亮随之消失（见 [`Job::highlight`]）。
//! 图片可以滚轮或右下角的按钮缩放；“复制全部”复制右边的文字，“保存图片”（⌘S）把图片存成 PNG。
//! 复制的内容照常记进剪贴板历史。

use std::ops::Range;
use std::sync::mpsc::{self, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{
    pos2, vec2, Align, Align2, Color32, CursorIcon, Event, FontId, Frame, Key, Layout, Margin,
    Modifiers, MouseWheelUnit, Pos2, Rect, RichText, Sense, Stroke, TextBuffer, TextureHandle,
    TextureOptions, Ui, Vec2,
};
use image::RgbaImage;

use super::fonts;
use super::icons::{self, Icon};
use super::style::{self, Palette};
use super::widgets::{centered_row, drag_window, icon_button, labeled_button};
use super::HubApp;
use crate::keys;
use crate::platform::{self, panel as native_panel, Anchor, Placement};
use crate::shared::{AppEvent, Shared};
use crate::snip::{self, ocr::Line};

/// 右边文字的字号。
const FONT_SIZE: f32 = 14.0;
/// 缩放比例（1.0 = 100%，一个图像像素对应一个物理像素）的上下限。
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 8.0;
/// 点一下缩放按钮改变的比例。
const ZOOM_STEP: f32 = 1.25;
/// 滚轮每“一格”（50 点，与贴图一致）改变的比例。
const WHEEL_STEP: f32 = 1.2;
const POINTS_PER_STEP: f32 = 50.0;
/// 缩放条离图片栏右下角的距离。
const ZOOM_BAR_MARGIN: f32 = 10.0;

/// 提取文字页面的状态：最近一次识别的图片和结果（窗口收起后保留）。
pub struct OcrView {
    job: Option<Job>,
    /// 固定窗口：失去焦点时不自动隐藏。
    pub pinned: bool,
    /// 图片栏占内容宽度的比例（拖动分隔条后更新；窗口大小变了按它重新分栏）。
    split: f32,
    /// 上一帧的内容宽度（用来发现窗口大小变化）。
    width: f32,
}

impl Default for OcrView {
    fn default() -> Self {
        Self {
            job: None,
            pinned: false,
            split: 0.5,
            width: 0.0,
        }
    }
}

/// 一次识别。
struct Job {
    image: Arc<RgbaImage>,
    texture: Option<TextureHandle>,
    state: State,
    /// 右边可以修改的文字（识别完成时填入）。
    text: String,
    /// 每一行在 `text` 里的字节范围（与 [`State::Done`] 的 `lines` 同序）；文字被改过之后为 `None`。
    spans: Option<Vec<Range<usize>>>,
    /// 图片上选中的文字。
    selection: Option<Selection>,
    /// 缩放比例；`None` 表示适应栏宽。
    zoom: Option<f32>,
    /// 上一帧“适应栏宽”的比例：窗口或分栏大小变了时据此按比例调整缩放。
    fit: f32,
    /// 图片栏上一帧的滚动位置（滚轮缩放时据此让鼠标下面的点停在原处）。
    offset: Vec2,
    /// 下一帧把右边标亮的文字滚到看得见的地方。
    reveal: bool,
}

/// 图片上选中的一段文字：从 `anchor`（按下的地方）到 `head`（拖到的地方），
/// 位置是（行下标，行内第几个字符之前）。两边都可以是端点，用得着的时候再看谁在前。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    anchor: (usize, usize),
    head: (usize, usize),
}

impl Selection {
    /// 起点在前的一套端点。
    fn ends(self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// 这一行要标亮的字符范围；不在这段里、或者没有字符时为空。
    fn chars_of(self, line: usize, count: usize) -> Option<Range<usize>> {
        let ((start, from), (end, to)) = self.ends();
        if line < start || line > end {
            return None;
        }
        let from = if line == start { from } else { 0 };
        let to = if line == end { to } else { count };
        (from < to).then_some(from..to)
    }
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

impl Job {
    /// 识别出的文字行（还没识别完时为空）。
    fn lines(&self) -> &[Line] {
        match &self.state {
            State::Done { lines, .. } => lines,
            State::Running { .. } | State::Failed(_) => &[],
        }
    }

    /// 选中的文字在 `text` 里的字节范围：没选中、文字被改过（`spans` 已清空）时为空。
    fn highlight(&self) -> Option<Range<usize>> {
        let spans = self.spans.as_ref()?;
        let lines = self.lines();
        let ((start, from), (end, to)) = self.selection?.ends();
        let first = spans.get(start)?;
        let last = spans.get(end)?;
        let from = first.start + trimmed_offset(&lines.get(start)?.text, from);
        let to = (last.start + trimmed_offset(&lines.get(end)?.text, to)).min(last.end);
        (from < to).then_some(from..to)
    }
}

/// 一行文字里第 `chars` 个字符之前的字节数；这一行拼进整段文本时去掉了首尾空白，这里一并减掉。
fn trimmed_offset(text: &str, chars: usize) -> usize {
    let lead = text.len() - text.trim_start().len();
    let bytes: usize = text.chars().take(chars).map(char::len_utf8).sum();
    bytes.saturating_sub(lead)
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
            spans: None,
            selection: None,
            zoom: None,
            fit: 0.0,
            offset: Vec2::ZERO,
            reveal: false,
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
                // 每一行落在文本里的位置（与 `lines` 同序），选中图片上的文字时据此标亮右边的文字。
                let (text, spans) = snip::ocr::to_text_with_spans(&lines);
                job.text = text;
                job.spans = Some(spans);
                job.selection = None;
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
    /// 复制一段文字（双击图片上的一行）。
    CopyLine(String),
    /// 复制右边的全部文字。
    CopyAll,
    /// 把图片存成 PNG。
    Save,
    Close,
}

impl HubApp {
    /// 提取图片里的文字：开始后台识别，主窗口切到提取文字页面（出现在屏幕中央）。
    pub(super) fn open_ocr(&mut self, image: Arc<RgbaImage>) {
        self.startup_hide_pending = false;
        self.cancel_hotkey_recording();
        self.ocr.start(image, self.shared.clone());
        self.set_page(super::Page::Ocr);
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
            if input.consume_key(Modifiers::COMMAND, Key::S) {
                actions.push(OcrAction::Save);
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
                // 分开借用：图片栏的比例要跟着更新。
                let OcrView {
                    job, split, width, ..
                } = &mut self.ocr;
                let Some(job) = job.as_mut() else {
                    return;
                };
                let total = ui.available_width();
                let (min, max) = (160.0, (total * 0.75).max(160.0));
                let mut panel = egui::Panel::left("ocr-image")
                    .resizable(true)
                    .size_range(min..=max)
                    .frame(
                        Frame::NONE
                            .fill(palette.field)
                            .inner_margin(Margin::same(8)),
                    );
                panel = if *width > 0.0 && (total - *width).abs() > 0.5 {
                    // 窗口大小变了：左右两栏按原来的比例重新分（图片也会跟着重新适应栏宽）。
                    panel.exact_size((total * *split).clamp(min, max))
                } else {
                    panel.default_size(total * *split)
                };
                let response = panel.show(ui, |ui| image_view(ui, job, &palette, &mut actions));
                if total > 0.0 {
                    // 记住分栏比例（用户拖动分隔条之后这里会变）。
                    *split = (response.response.rect.width() / total).clamp(0.05, 0.95);
                }
                *width = total;
                text_view(ui, job, &palette);
            });

        self.apply_ocr_actions(actions);
    }

    /// 标题栏：图标、标题，右侧是固定按钮；空白处可以拖动窗口。
    ///
    /// macOS 的窗口把关闭按钮放在左上角（一个红点，与系统的红绿灯一致），其他平台放在右上角。
    fn ocr_header(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<OcrAction>) {
        drag_window(ui, ui.max_rect());
        centered_row(ui, 28.0, |ui| {
            #[cfg(target_os = "macos")]
            {
                if super::widgets::traffic_light_close(ui)
                    .on_hover_text(format!("关闭  {}", keys::ESCAPE))
                    .clicked()
                {
                    actions.push(OcrAction::Close);
                }
                ui.add_space(4.0);
            }
            #[cfg(not(target_os = "macos"))]
            {
                let (badge, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                ui.painter()
                    .rect_filled(badge, style::SMALL_RADIUS, palette.accent);
                icons::paint_centered(ui.painter(), badge, 14.0, Icon::Ocr, Color32::WHITE);
            }
            ui.label(
                RichText::new("提取文字")
                    .size(14.0)
                    .family(egui::FontFamily::Name(fonts::SEMIBOLD.into()))
                    .color(palette.text),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                #[cfg(not(target_os = "macos"))]
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

    /// 底栏：左边是识别状态，右边是“复制全部”“保存”。
    fn ocr_footer(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<OcrAction>) {
        centered_row(ui, 28.0, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let has_text = self
                    .ocr
                    .job
                    .as_ref()
                    .is_some_and(|job| !job.text.trim().is_empty());
                if labeled_button(
                    ui,
                    Some(Icon::Save),
                    "保存图片",
                    self.ocr.job.is_some(),
                    palette,
                )
                    .on_hover_text(format!(
                        "把左边的图片存成 PNG  {}",
                        keys::shortcut(&[keys::COMMAND, "S"])
                    ))
                    .clicked()
                {
                    actions.push(OcrAction::Save);
                }
                if labeled_button(ui, Some(Icon::Copy), "复制全部", has_text, palette)
                    .on_hover_text(format!(
                        "复制右边的文字  {}",
                        keys::shortcut(&[keys::SHIFT, keys::COMMAND, "C"])
                    ))
                    .clicked()
                {
                    actions.push(OcrAction::CopyAll);
                }
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
                OcrAction::Save => {
                    let Some(image) = self.ocr.job.as_ref().map(|job| job.image.clone()) else {
                        continue;
                    };
                    // 不把焦点还给之前的应用：对话框关掉后面板要接着用（见 `floating::save_dialog`）。
                    if self.save_image(&image, false) {
                        self.show_toast("已保存");
                    }
                    // 让窗口重新成为键盘窗口，否则它看上去就像被关掉了，接下来的 esc、⌘S 也收不到。
                    native_panel::refocus();
                    self.was_focused = native_panel::is_active();
                    // 对话框收尾时的焦点变化不算用户点了别的应用：下一次失焦不收起窗口。
                    self.just_closed_dialog = true;
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
                "识别出 {} 行 · 用时 {:.1} 秒 · 在图片上拖动可以选中文字",
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

/// 左边的图片：滚轮 / 右下角的按钮缩放；在文字上拖动选中其中的一部分（右边同步标亮）。
/// 识别出的文字不画框，只有选中的部分铺一层强调色。
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
    let pixels = vec2(job.image.width() as f32, job.image.height() as f32);
    // 1 个图像像素对应 1 个物理像素，即 100%。
    let natural = pixels / ctx.pixels_per_point();
    // 栏里放得下的比例（不放大），也就是默认的“适应宽度”。
    let fit = (ui.available_width() / natural.x).min(1.0);
    let viewport = ui.available_rect_before_wrap();
    let mut zoom = job.zoom.unwrap_or(fit);
    // 窗口或分栏大小变了：原来缩放过的话按同样的比例跟着变，图片重新适应新的栏宽。
    if job.zoom.is_some() && job.fit > 0.0 && (job.fit - fit).abs() > 1e-4 {
        zoom = (zoom * fit / job.fit).clamp(MIN_ZOOM, MAX_ZOOM);
        job.zoom = Some(zoom);
    }
    job.fit = fit;
    let anchor = ctx
        .input(|i| i.pointer.hover_pos())
        .filter(|pos| viewport.contains(*pos));
    // 滚轮缩放：以鼠标位置为锚点，缩放前后鼠标下面的那个图像点停在原处。
    let factor = wheel_zoom(&ctx, anchor.is_some());
    let mut offset = None;
    if (factor - 1.0).abs() > 1e-4 {
        let cursor = anchor.unwrap_or_else(|| viewport.center()) - viewport.min;
        let zoomed = (zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let max = (natural * zoomed - viewport.size()).max(Vec2::ZERO);
        offset = Some(anchored_offset(cursor, job.offset, zoom, zoomed, max));
        zoom = zoomed;
        job.zoom = Some(zoomed);
        ctx.request_repaint();
    }
    let size = natural * zoom;
    let mut area = egui::ScrollArea::both()
        .id_salt("ocr-image-scroll")
        .auto_shrink([false, false])
        // 滚轮用来缩放，滚动条用来移动位置，拖动留给选择文字。
        .scroll_source(egui::scroll_area::ScrollSource {
            scroll_bar: true,
            drag: egui::scroll_area::DragScroll::Never,
            mouse_wheel: false,
        });
    if let Some(offset) = offset {
        area = area.scroll_offset(offset);
    }
    let output = area.show(ui, |ui| {
        // 分开借用：`lines` 与图片只读，选中的范围要改。
        let Job {
            image,
            state,
            selection,
            reveal,
            ..
        } = job;
        let lines = match state {
            State::Done { lines, .. } => lines.as_slice(),
            State::Running { .. } | State::Failed(_) => &[],
        };
        let (outer, response) =
            ui.allocate_exact_size(size.max(ui.available_size()), Sense::click_and_drag());
        // 图片放得下时水平居中、顶边对齐（与微信一致），放不下时从左上角开始，交给滚动条。
        let rect = Rect::from_min_size(pos2(outer.center().x - size.x / 2.0, outer.top()), size);
        let painter = ui.painter_at(rect);
        if snip::has_alpha(image) {
            painter.rect_filled(rect, 0.0, Color32::WHITE);
        }
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        // 图像像素 → 屏幕坐标（100% 时 1 个图像像素是 1 个物理像素）。
        let scale = size.x / pixels.x;
        let pointer = response.interact_pointer_pos();
        let caret = pointer.and_then(|pos| caret_at(lines, rect.min, scale, pos));
        // 按下时记住起点，拖到哪个字就是在这一行里选中哪几个字。
        // 拖动要移动超过阈值才开始，`interact_pointer_pos` 已经是拖到的位置，起点要从按下的位置算。
        if response.drag_started() {
            let press = ctx.input(|i| i.pointer.press_origin());
            if let Some(anchor) = press.and_then(|pos| caret_at(lines, rect.min, scale, pos)) {
                *selection = Some(Selection {
                    anchor,
                    head: caret.unwrap_or(anchor),
                });
            }
        }
        if response.dragged() {
            if let (Some(selection), Some(head)) = (selection.as_mut(), caret) {
                selection.head = head;
                *reveal = true;
            }
        }
        if response.clicked() {
            // 单击选中整行（在右边快速定位）；点在文字外面取消选择。
            *selection = caret.map(|(line, _)| Selection {
                anchor: (line, 0),
                head: (line, lines[line].chars.len()),
            });
            *reveal = true;
        }
        if let Some(selection) = *selection {
            for (index, line) in lines.iter().enumerate() {
                let Some(range) = selection.chars_of(index, line.chars.len()) else {
                    continue;
                };
                let Some(cells) = line.chars.get(range) else {
                    continue;
                };
                let (Some(first), Some(last)) = (cells.first(), cells.last()) else {
                    continue;
                };
                let band = screen_rect(line.rect, rect.min, scale);
                let left = screen_rect(*first, rect.min, scale).left();
                let right = screen_rect(*last, rect.min, scale).right().max(left + 2.0);
                let area = Rect::from_min_max(
                    pos2(left, band.top()),
                    pos2(right.min(band.right()), band.bottom()),
                );
                painter.rect_filled(area.expand(1.0), 2.0, palette.accent.gamma_multiply(0.30));
            }
        }
        if let Some((index, _)) = caret {
            ctx.set_cursor_icon(CursorIcon::Text);
            // 双击复制这一行（单击是选中整行，便于在右边定位）。
            if response.double_clicked() {
                actions.push(OcrAction::CopyLine(lines[index].text.clone()));
            }
        }
    });
    job.offset = output.state.offset;
    zoom_bar(ui, viewport, zoom, job, palette);
}

/// 图片里的矩形在屏幕上的位置：`origin` 是图片左上角，`scale` 是图像像素到点的比例。
fn screen_rect(rect: Rect, origin: Pos2, scale: f32) -> Rect {
    Rect::from_min_max(
        origin + rect.min.to_vec2() * scale,
        origin + rect.max.to_vec2() * scale,
    )
}

/// `pos`（屏幕坐标）落在哪一行文字上；允许稍微超出一点，点击更容易命中。
fn line_at(lines: &[Line], origin: Pos2, scale: f32, pos: Pos2) -> Option<usize> {
    lines.iter().position(|line| {
        screen_rect(line.rect, origin, scale)
            .expand(2.0)
            .contains(pos)
    })
}

/// `pos`（屏幕坐标）落在哪个位置：行下标，以及这一行的第几个字符之前。
///
/// 落在某个字的左半边算这个字之前，右半边算下一个字之前（与文字光标一样）。
fn caret_at(lines: &[Line], origin: Pos2, scale: f32, pos: Pos2) -> Option<(usize, usize)> {
    let index = line_at(lines, origin, scale, pos)?;
    let cells = &lines[index].chars;
    let caret = cells
        .iter()
        .position(|cell| pos.x < screen_rect(*cell, origin, scale).center().x)
        .unwrap_or(cells.len());
    Some((index, caret))
}

/// 缩放之后让鼠标下面那个图像点停在原处的滚动偏移。
///
/// `cursor` 是鼠标相对视口左上角的位置，`offset` 是当前的滚动偏移，缩放比例从 `zoom` 变到 `next`，
/// `max` 是缩放之后的最大偏移。
fn anchored_offset(cursor: Vec2, offset: Vec2, zoom: f32, next: f32, max: Vec2) -> Vec2 {
    let point = (cursor + offset) / zoom;
    (point * next - cursor).clamp(Vec2::ZERO, max)
}

/// 滚轮 / 触控板捏合产生的缩放倍数（没有时是 1.0）。
fn wheel_zoom(ctx: &egui::Context, hovered: bool) -> f32 {
    if !hovered {
        return 1.0;
    }
    ctx.input(|i| {
        let mut steps = 0.0;
        let mut pinch = 1.0;
        for event in &i.events {
            match event {
                // ⌘ / ⇧ 等修饰键的滚轮留给滚动条，不带修饰键的才缩放。
                Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } if !modifiers.any() => {
                    steps += match unit {
                        MouseWheelUnit::Point => delta.y / POINTS_PER_STEP,
                        MouseWheelUnit::Line => delta.y,
                        MouseWheelUnit::Page => delta.y * 3.0,
                    };
                }
                Event::Zoom(factor) => pinch *= factor,
                _ => {}
            }
        }
        WHEEL_STEP.powf(steps) * pinch
    })
}

/// 图片栏右下角的缩放条：缩小、比例（点一下恢复适应宽度）、放大。
fn zoom_bar(ui: &mut Ui, viewport: Rect, zoom: f32, job: &mut Job, palette: &Palette) {
    const HEIGHT: f32 = 30.0;
    const WIDTH: f32 = 100.0;
    let rect = Rect::from_min_size(
        viewport.max - vec2(WIDTH + ZOOM_BAR_MARGIN, HEIGHT + ZOOM_BAR_MARGIN),
        vec2(WIDTH, HEIGHT),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        Frame::new()
            .fill(palette.background)
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(style::CAPSULE)
            .inner_margin(Margin::symmetric(2, 2))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if icon_button(ui, Icon::ZoomOut, false, palette)
                        .on_hover_text("缩小")
                        .clicked()
                    {
                        job.zoom = Some((zoom / ZOOM_STEP).clamp(MIN_ZOOM, MAX_ZOOM));
                    }
                    let percent = ui.allocate_exact_size(vec2(38.0, 24.0), Sense::click());
                    let (rect, response) = percent;
                    if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, style::CAPSULE, palette.field);
                    }
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        format!("{}%", (zoom * 100.0).round() as i32),
                        FontId::proportional(11.5),
                        palette.weak,
                    );
                    if response.on_hover_text("点一下恢复适应宽度").clicked() {
                        job.zoom = None;
                    }
                    if icon_button(ui, Icon::ZoomIn, false, palette)
                        .on_hover_text("放大（也可以把鼠标放在图片上滚动滚轮）")
                        .clicked()
                    {
                        job.zoom = Some((zoom * ZOOM_STEP).clamp(MIN_ZOOM, MAX_ZOOM));
                    }
                });
            });
    });
}

/// 右边的文字：可以修改、选中复制；识别中、没有结果时显示提示。
/// 图片上选中的几行在这里标亮，改动文字后标亮消失。
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
    let highlight = job.highlight();
    let Job {
        text,
        selection,
        spans,
        reveal,
        ..
    } = job;
    let reveal = std::mem::replace(reveal, false);
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
                    let output = {
                        let mut layouter = |ui: &Ui, buffer: &dyn TextBuffer, wrap_width: f32| {
                            let layout = text_layout(
                                buffer.as_str(),
                                highlight.clone(),
                                palette,
                                wrap_width,
                            );
                            ui.fonts_mut(|f| f.layout_job(layout))
                        };
                        egui::TextEdit::multiline(text)
                            .font(FontId::proportional(FONT_SIZE))
                            .text_color(palette.text)
                            .frame(Frame::NONE)
                            .margin(Margin::ZERO)
                            .min_size(ui.available_size())
                            .layouter(&mut layouter)
                            .show(ui)
                    };
                    if output.response.changed() {
                        // 右边的文字改了：识别出的位置已经对不上，标亮一起丢掉。
                        *selection = None;
                        *spans = None;
                    }
                    // 在图片上选了一段：把右边对应的文字滚到看得见的地方。
                    if reveal {
                        if let Some(range) = highlight.filter(|_| selection.is_some()) {
                            let start = char_index(text, range.start);
                            let pos = output
                                .galley
                                .pos_from_cursor(CCursor::new(start))
                                .translate(output.galley_pos.to_vec2());
                            ui.scroll_to_rect(pos.expand2(vec2(60.0, 80.0)), Some(Align::Center));
                        }
                    }
                });
        });
}

/// 字节偏移 → 字符个数（egui 的光标按字符计数）。
fn char_index(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    text[..byte].chars().count()
}

/// 右边文字的排版：`highlight` 那段文字铺一层强调色的底（与图片上选中的几行对应）。
fn text_layout(
    text: &str,
    highlight: Option<Range<usize>>,
    palette: &Palette,
    wrap_width: f32,
) -> LayoutJob {
    let format = TextFormat {
        font_id: FontId::proportional(FONT_SIZE),
        color: palette.text,
        ..Default::default()
    };
    let mut layout = LayoutJob::default();
    layout.wrap.max_width = wrap_width;
    let range = highlight.filter(|range| {
        range.start < range.end
            && range.end <= text.len()
            && text.is_char_boundary(range.start)
            && text.is_char_boundary(range.end)
    });
    let Some(range) = range else {
        layout.append(text, 0.0, format);
        return layout;
    };
    let highlighted = TextFormat {
        background: palette.accent.gamma_multiply(0.26),
        ..format.clone()
    };
    layout.append(&text[..range.start], 0.0, format.clone());
    layout.append(&text[range.clone()], 0.0, highlighted);
    layout.append(&text[range.end..], 0.0, format);
    layout
}

#[cfg(test)]
impl OcrView {
    /// 测试用：不经系统识别，直接给出图片和识别结果；`lines` 为空时停在“正在识别”。
    pub(super) fn set_result(&mut self, image: RgbaImage, lines: Option<Vec<Line>>) {
        let (sender, receiver) = mpsc::channel();
        match lines {
            Some(mut lines) => {
                // 与真实识别一样补齐每个字符的位置。
                snip::ocr::fill_chars(&mut lines);
                sender.send(Ok(lines)).unwrap();
            }
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
            spans: None,
            selection: None,
            zoom: None,
            fit: 0.0,
            offset: Vec2::ZERO,
            reveal: false,
        });
        self.poll();
    }

    /// 右边的文字。
    pub(super) fn text(&self) -> Option<&str> {
        self.job.as_ref().map(|job| job.text.as_str())
    }

    /// 测试用：在图片上选中第 `index` 行的全部文字。
    pub(super) fn select_line(&mut self, index: usize) {
        let Some(job) = &mut self.job else {
            return;
        };
        let count = job.lines().get(index).map_or(0, |line| line.chars.len());
        self.select(index, 0, index, count);
    }

    /// 测试用：选中第 `index` 行里从第 `from` 个字符到第 `to` 个字符（不含）。
    pub(super) fn select_chars(&mut self, index: usize, from: usize, to: usize) {
        self.select(index, from, index, to);
    }

    fn select(&mut self, start: usize, from: usize, end: usize, to: usize) {
        let Some(job) = &mut self.job else {
            return;
        };
        job.selection = Some(Selection {
            anchor: (start, from),
            head: (end, to),
        });
        job.reveal = true;
    }

    /// 测试用：图片上选中的几行在右边文字里的位置。
    pub(super) fn highlight(&self) -> Option<Range<usize>> {
        self.job.as_ref().and_then(Job::highlight)
    }

    /// 测试用：当前“适应栏宽”的比例（窗口大小变了会变）。
    pub(super) fn fit(&self) -> f32 {
        self.job.as_ref().map_or(0.0, |job| job.fit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f32, y: f32, w: f32, h: f32) -> Line {
        Line::new(text, Rect::from_min_size(pos2(x, y), vec2(w, h)))
    }

    #[test]
    fn finds_the_line_under_the_pointer() {
        let lines = vec![
            line("第一行", 10.0, 10.0, 100.0, 20.0),
            line("第二行", 10.0, 50.0, 100.0, 20.0),
        ];
        let origin = pos2(100.0, 100.0);
        // 100% 时图像像素就是点。
        assert_eq!(line_at(&lines, origin, 1.0, pos2(150.0, 120.0)), Some(0));
        assert_eq!(line_at(&lines, origin, 1.0, pos2(150.0, 160.0)), Some(1));
        assert_eq!(line_at(&lines, origin, 1.0, pos2(150.0, 300.0)), None);
        // 缩小到一半：图像上的 (30, 20) 在屏幕上是 (100 + 15, 100 + 10)。
        assert_eq!(line_at(&lines, origin, 0.5, pos2(115.0, 110.0)), Some(0));
    }

    #[test]
    fn caret_follows_the_character_under_the_pointer() {
        // 一行 4 个字符平分 100 像素：每个字 25 像素，中线在 12.5、37.5、62.5、87.5。
        let mut lines = vec![Line::new(
            "中文ab",
            Rect::from_min_size(pos2(10.0, 10.0), vec2(100.0, 20.0)),
        )];
        snip::ocr::fill_chars(&mut lines);
        let origin = pos2(0.0, 0.0);
        // 第一个字的左半边 → 第一个字之前。
        assert_eq!(
            caret_at(&lines, origin, 1.0, pos2(15.0, 20.0)),
            Some((0, 0))
        );
        // 第一个字的右半边 → 第二个字之前。
        assert_eq!(
            caret_at(&lines, origin, 1.0, pos2(40.0, 20.0)),
            Some((0, 1))
        );
        // 最后一个字之后。
        assert_eq!(
            caret_at(&lines, origin, 1.0, pos2(105.0, 20.0)),
            Some((0, 4))
        );
        // 不在这一行上就没有位置。
        assert_eq!(caret_at(&lines, origin, 1.0, pos2(40.0, 200.0)), None);
    }

    #[test]
    fn selection_covers_the_characters_it_spans() {
        let whole = Selection {
            anchor: (1, 2),
            head: (3, 1),
        };
        // 中间的行整行选中，两头的行只选一部分。
        assert_eq!(whole.chars_of(0, 5), None);
        assert_eq!(whole.chars_of(1, 5), Some(2..5));
        assert_eq!(whole.chars_of(2, 5), Some(0..5));
        assert_eq!(whole.chars_of(3, 5), Some(0..1));
        assert_eq!(whole.chars_of(4, 5), None);
        // 反过来拖是一样的。
        let reversed = Selection {
            anchor: (3, 1),
            head: (1, 2),
        };
        assert_eq!(reversed.ends(), whole.ends());
        assert_eq!(reversed.chars_of(3, 5), Some(0..1));
    }

    #[test]
    fn offsets_skip_the_trimmed_leading_space() {
        // 拼进整段文本时去掉行首空白：第 3 个字符（“文”）在文本里是第 3 个字节。
        assert_eq!(trimmed_offset("  中文", 3), 3);
        assert_eq!(trimmed_offset("中文", 1), 3);
        assert_eq!(trimmed_offset("  中文", 0), 0);
        assert_eq!(trimmed_offset("abc", 2), 2);
    }

    #[test]
    fn keeps_the_point_under_the_cursor_when_zooming() {
        // 视口 400×300，鼠标在 (100, 80)，滚动到 (50, 40)：鼠标下面是图像上的 (150, 120)。
        let cursor = vec2(100.0, 80.0);
        let max = vec2(1000.0, 1000.0);
        // 放大到 200%：那个点要停在原处，偏移变成 (300 - 100, 240 - 80)。
        assert_eq!(
            anchored_offset(cursor, vec2(50.0, 40.0), 1.0, 2.0, max),
            vec2(200.0, 160.0)
        );
        // 缩回 100%：回到原来的偏移。
        assert_eq!(
            anchored_offset(cursor, vec2(200.0, 160.0), 2.0, 1.0, max),
            vec2(50.0, 40.0)
        );
        // 偏移不会超出内容（左上角不会露出来）。
        assert_eq!(
            anchored_offset(vec2(10.0, 10.0), Vec2::ZERO, 1.0, 0.5, max),
            Vec2::ZERO
        );
    }

    #[test]
    fn highlights_the_selected_lines_in_the_text() {
        let palette = Palette::new(true);
        let text = "一二\n三四";
        let layout = text_layout(text, Some(3..6), &palette, 400.0);
        let marked: Vec<_> = layout
            .sections
            .iter()
            .filter(|section| section.format.background != Color32::TRANSPARENT)
            .collect();
        assert_eq!(marked.len(), 1);
        let range = marked[0].byte_range.clone();
        assert_eq!((range.start.0, range.end.0), (3, 6));
        assert_eq!(&text[range.start.0..range.end.0], "二");
        // 范围不合法（不是字符边界、超出文本）时不标亮，也不会 panic。
        for range in [7..9, 0..100, 6..6] {
            let layout = text_layout(text, Some(range), &palette, 400.0);
            assert_eq!(layout.sections.len(), 1);
        }
        let layout = text_layout(text, None, &palette, 400.0);
        assert_eq!(layout.sections.len(), 1);
    }
}
