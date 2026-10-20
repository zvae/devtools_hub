//! 滚动截图（长截图），参照微信的长截图（分析见 `docs/微信滚动截图与文字提取实现分析.md`）。
//!
//! 选好区域后点工具栏上的“长截图”进入：
//! - 遮罩不再显示静止的截图（窗口是透明的，看到的是真实的屏幕），只在选区外加遮罩、画边框；
//! - 鼠标在选区里时遮罩让鼠标穿透（`ViewportCommand::MousePassthrough`，macOS 上即 `setIgnoresMouseEvents:`），
//!   用户直接滚动下面的窗口；鼠标移到选区外时恢复，可以点工具栏。穿透期间遮罩收不到鼠标事件，
//!   所以定时（[`POLL`]）重绘，用平台层读鼠标位置；
//! - 后台线程每隔 [`INTERVAL`] 截一次选区（只截遮罩下面的窗口，遮罩上的边框、工具栏、预览不会进入画面），
//!   交给 [`Stitcher`] 拼接，旁边显示拼好的长图的预览；
//! - 退出、贴图、保存、复制与普通截屏一样（esc、⌘T、⌘S、⌘C / ↩），输出拼好的长图。
//!
//! 与微信一样不模拟滚轮：由用户自己滚动。
//!
//! 排查拼接问题：设置环境变量 `DEVTOOLS_HUB_SCROLL_DUMP=<目录>` 后，每一帧都存成 PNG 并在日志里记下拼接结果，
//! 可以用 `snip::scroll` 测试里的 `replay_frames` 回放。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use egui::{
    pos2, vec2, Color32, Context, Key, Modifiers, Painter, Rect, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, ViewportCommand, ViewportId,
};
use image::RgbaImage;

use super::toolbar::{self, BarAction, BarState, Mode};
use super::{badge, paint_mask, Outcome, Output, SnipSession, View, ACCENT};
use crate::platform::screen;
use crate::snip::scroll::{Step, Stitcher};
use crate::snip::{self, Capture};

/// 后台线程截取选区的周期：两帧之间滚动的距离不能超过选区的高度，滚得快时要截得勤
/// （截取和拼接本身要十几毫秒，超出周期时紧接着截下一帧）。
const INTERVAL: Duration = Duration::from_millis(30);
/// 鼠标穿透期间多久重绘一次（读鼠标位置，决定是否穿透）。
const POLL: Duration = Duration::from_millis(33);
/// 边框画在选区外这么远（点），截取选区时不会截到边框。
const BORDER_GAP: f32 = 2.0;
/// 预览的宽度（点）。
const PREVIEW_WIDTH: f32 = 140.0;
/// 预览与选区的间距（点）。
const PREVIEW_GAP: f32 = 8.0;
/// 预览图最多保留这么多行（像素，足够铺满一块屏幕的高度）。
const PREVIEW_ROWS: u32 = 2400;
/// 能进入滚动截图的最小选区（像素）：太小的区域里没有足够的内容判断滚动了多少。
pub(super) const MIN_SIZE: f32 = 64.0;

/// 截取选区的函数：每调用一次返回选区现在的画面（物理像素，大小与选区相同）。
pub(super) type FrameSource = Box<dyn FnMut() -> Result<RgbaImage> + Send>;

/// 为选区创建 [`FrameSource`]：`capture` 是截屏，`rect` 是选区（像素），`below` 是遮罩的窗口编号。
/// 测试里换成模拟的画面。
pub(super) type FrameFactory = fn(&Capture, Rect, Option<u32>) -> FrameSource;

/// 拼接的进展（界面上的提示）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Status {
    /// 还没截到第一帧。
    #[default]
    Starting,
    /// 截到了第一帧，等用户滚动。
    Ready,
    /// 正在拼接。
    Scrolling,
    /// 回到了已经拼过的位置（往回滚了）。
    Behind,
    /// 最近一帧接不上：滚得太快，或者画面整个变了。
    Lost,
    /// 长图达到了最大高度。
    Full,
    /// 截取失败（例如没有屏幕录制权限）。
    Failed,
}

/// 后台线程交给界面的进展。
#[derive(Default)]
struct Progress {
    status: Status,
    /// 长图的大小（像素）。
    size: [u32; 2],
    /// 预览图（每次拼上新内容后更新）与它的版本号。
    preview: Option<RgbaImage>,
    version: u64,
}

/// 一次滚动截图：后台截取、拼接的线程与界面上的状态。
pub(super) struct ScrollCapture {
    progress: Arc<Mutex<Progress>>,
    /// 拼接器；后台线程每拼一帧锁一次，结束时界面取走拼好的长图。
    stitcher: Arc<Mutex<Option<Stitcher>>>,
    stop: Arc<AtomicBool>,
    texture: Option<(u64, TextureHandle)>,
    /// 现在是否让鼠标穿透遮罩。
    passthrough: bool,
}

impl ScrollCapture {
    /// 开始在后台截取、拼接。每处理完一帧请求重绘遮罩 `viewport`。
    pub(super) fn start(mut source: FrameSource, ctx: Context, viewport: ViewportId) -> Self {
        let progress = Arc::new(Mutex::new(Progress::default()));
        let stitcher = Arc::new(Mutex::new(None::<Stitcher>));
        let stop = Arc::new(AtomicBool::new(false));
        let (progress_in, stitcher_in, stop_in) =
            (progress.clone(), stitcher.clone(), stop.clone());
        thread::Builder::new()
            .name("scroll-capture".into())
            .spawn(move || {
                let mut started = std::time::Instant::now();
                let dump =
                    std::env::var_os("DEVTOOLS_HUB_SCROLL_DUMP").map(std::path::PathBuf::from);
                let mut index = 0usize;
                let mut previous: Option<RgbaImage> = None;
                while !stop_in.load(Ordering::Relaxed) {
                    let frame = match source() {
                        Ok(frame) => frame,
                        Err(err) => {
                            tracing::warn!("滚动截图截取失败：{err:#}");
                            progress_in.lock().unwrap().status = Status::Failed;
                            ctx.request_repaint_of(viewport);
                            return;
                        }
                    };
                    // 画面没变（没有滚动）时不用再算一遍。
                    if previous.as_ref() != Some(&frame) {
                        previous = Some(frame.clone());
                        if let Some(dir) = &dump {
                            let path = dir.join(format!("{index:04}.png"));
                            if let Err(err) = std::fs::create_dir_all(dir)
                                .map_err(anyhow::Error::from)
                                .and_then(|()| frame.save(&path).map_err(anyhow::Error::from))
                            {
                                tracing::warn!(
                                    "保存滚动截图的画面失败（{}）：{err:#}",
                                    path.display()
                                );
                            }
                        }
                        let mut stitcher = stitcher_in.lock().unwrap();
                        if stop_in.load(Ordering::Relaxed) {
                            return;
                        }
                        // 新的提示状态（`None` 表示不变）与长图是否变了。
                        let (status, changed) = match stitcher.as_mut() {
                            Some(stitcher) => {
                                let step = stitcher.push(frame);
                                if dump.is_some() {
                                    tracing::info!("滚动截图第 {index} 帧：{step:?}");
                                }
                                match step {
                                    Step::Unchanged => (None, false),
                                    Step::Appended { .. } => (Some(Status::Scrolling), true),
                                    Step::Behind => (Some(Status::Behind), false),
                                    Step::Lost => (Some(Status::Lost), false),
                                    Step::Full => (Some(Status::Full), true),
                                }
                            }
                            None => {
                                *stitcher = Some(Stitcher::new(frame));
                                (Some(Status::Ready), true)
                            }
                        };
                        let update = stitcher.as_ref().filter(|_| changed).map(|stitcher| {
                            let size = [stitcher.width(), stitcher.height()];
                            (size, stitcher.preview(PREVIEW_ROWS))
                        });
                        drop(stitcher);
                        let mut progress = progress_in.lock().unwrap();
                        if let Some(status) = status {
                            // 已经满了就一直显示“满了”。
                            if progress.status != Status::Full {
                                progress.status = status;
                            }
                        }
                        if let Some((size, preview)) = update {
                            progress.size = size;
                            progress.preview = Some(preview);
                            progress.version += 1;
                        }
                        drop(progress);
                        ctx.request_repaint_of(viewport);
                        index += 1;
                    }
                    thread::sleep(INTERVAL.saturating_sub(started.elapsed()));
                    started = std::time::Instant::now();
                }
            })
            .expect("创建滚动截图线程失败");
        Self {
            progress,
            stitcher,
            stop,
            texture: None,
            passthrough: false,
        }
    }

    /// 停止截取，取走拼好的长图（还没截到画面时为空）。
    pub(super) fn finish(&mut self) -> Option<RgbaImage> {
        self.stop.store(true, Ordering::Relaxed);
        // 后台线程正在拼接时等它拼完这一帧。
        let image = self.stitcher.lock().unwrap().as_ref().map(Stitcher::image);
        image
    }

    /// 拼接的进展与长图的大小。
    pub(super) fn status(&self) -> (Status, [u32; 2]) {
        let progress = self.progress.lock().unwrap();
        (progress.status, progress.size)
    }

    /// 预览图的纹理（有新的预览时重新上传）。
    fn preview_texture(&mut self, ctx: &Context) -> Option<TextureHandle> {
        let progress = self.progress.lock().unwrap();
        let outdated = self
            .texture
            .as_ref()
            .is_none_or(|(version, _)| *version != progress.version);
        if outdated {
            if let Some(preview) = &progress.preview {
                let texture = ctx.load_texture(
                    "snip-scroll-preview",
                    snip::color_image(preview),
                    TextureOptions::LINEAR,
                );
                self.texture = Some((progress.version, texture));
            }
        }
        self.texture.as_ref().map(|(_, texture)| texture.clone())
    }

    /// 鼠标在选区里时让鼠标穿透遮罩，移出选区后恢复。
    fn set_passthrough(&mut self, ctx: &Context, passthrough: bool) {
        if self.passthrough != passthrough {
            self.passthrough = passthrough;
            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(passthrough));
        }
    }
}

impl Drop for ScrollCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// 平台的截取方式：截取选区所在的桌面区域（只截遮罩 `below` 下面的窗口），再裁成与选区一样大。
pub(super) fn platform_source(capture: &Capture, rect: Rect, below: Option<u32>) -> FrameSource {
    let [frame_x, frame_y, frame_width, _] = capture.display.frame;
    // 每个点对应的像素数（与截屏一致）。
    let k = f64::from(capture.image.width()) / frame_width;
    // 截取范围取整到整点，再从截到的图像里裁出选区：Retina 屏上选区的边可能落在半个点上。
    let (x0, y0) = (
        (f64::from(rect.min.x) / k).floor(),
        (f64::from(rect.min.y) / k).floor(),
    );
    let (x1, y1) = (
        (f64::from(rect.max.x) / k).ceil(),
        (f64::from(rect.max.y) / k).ceil(),
    );
    let area = [frame_x + x0, frame_y + y0, x1 - x0, y1 - y0];
    let expected = [
        ((x1 - x0) * k).round() as u32,
        ((y1 - y0) * k).round() as u32,
    ];
    let offset = [
        (f64::from(rect.min.x) - x0 * k).round() as u32,
        (f64::from(rect.min.y) - y0 * k).round() as u32,
    ];
    let size = [rect.width() as u32, rect.height() as u32];
    Box::new(move || {
        let image = screen::capture_area(area, below)?;
        Ok(fit_frame(image, expected, offset, size))
    })
}

/// 把截到的图像变成选区的画面：大小与预期（`expected`）不同时先缩放到预期大小，再从 `offset` 处裁出 `size`。
fn fit_frame(image: RgbaImage, expected: [u32; 2], offset: [u32; 2], size: [u32; 2]) -> RgbaImage {
    let image = if image.dimensions() != (expected[0], expected[1]) {
        image::imageops::resize(
            &image,
            expected[0].max(1),
            expected[1].max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };
    let (width, height) = (size[0].min(image.width()), size[1].min(image.height()));
    let x = offset[0].min(image.width() - width);
    let y = offset[1].min(image.height() - height);
    if (x, y, width, height) == (0, 0, image.width(), image.height()) {
        return image;
    }
    image::imageops::crop_imm(&image, x, y, width, height).to_image()
}

impl SnipSession {
    /// 能否进入滚动截图：支持的平台、不是编辑贴图、没有标注、选区不太小。
    pub(super) fn can_scroll(&self) -> bool {
        crate::platform::SCROLL_CAPTURE_SUPPORTED
            && self.editing.is_none()
            && self.marks.is_empty()
            && self
                .selection
                .is_some_and(|rect| rect.width() >= MIN_SIZE && rect.height() >= MIN_SIZE)
    }

    /// 进入滚动截图：开始在后台截取选区。
    pub(super) fn start_scroll(&mut self, ctx: &Context, view: &View) {
        if !self.can_scroll() {
            return;
        }
        let Some(selection) = self.selection else {
            return;
        };
        self.commit_text(ctx, view);
        self.tool = None;
        self.draft = None;
        self.drag = None;
        let below = crate::platform::floating::overlay_window_id(&self.title);
        if below.is_none() {
            tracing::warn!("没找到截屏遮罩的窗口编号，滚动截图可能截到遮罩");
        }
        let source = (self.frame_source)(&self.capture, selection, below);
        tracing::info!("开始滚动截图：选区 {selection:?}");
        self.scroll = Some(ScrollCapture::start(
            source,
            ctx.clone(),
            self.viewport_id(),
        ));
        ctx.request_repaint();
    }

    /// 滚动截图中的界面：真实屏幕上的遮罩、边框、工具栏、提示与预览。
    pub(super) fn scroll_ui(&mut self, ui: &mut Ui, ctx: &Context) -> Outcome {
        let screen_rect = ctx.viewport_rect();
        let view = View::new(screen_rect, self.capture.bounds());
        let Some(selection) = self.selection else {
            return Outcome::Cancel;
        };
        let hole = view.to_points(selection);
        let frame = hole.expand(BORDER_GAP);
        let painter = ui.painter().with_clip_rect(screen_rect);

        // 鼠标在选区里时穿透（遮罩收不到鼠标事件，用平台层读鼠标位置）。
        let inside = screen::cursor()
            .map(|(point, _)| self.capture.global_to_pixels(point))
            .is_some_and(|point| selection.contains(point));
        let (status, size) = {
            let Some(scroll) = &mut self.scroll else {
                return Outcome::Cancel;
            };
            scroll.set_passthrough(ctx, inside);
            scroll.status()
        };

        paint_mask(&painter, screen_rect, frame);
        painter.rect_stroke(frame, 0.0, Stroke::new(1.5, ACCENT), StrokeKind::Outside);
        badge(&painter, screen_rect, frame, &status_text(status, size));
        self.paint_preview(ctx, &painter, screen_rect, frame);

        let bars = toolbar::layout(frame, screen_rect, None, Mode::Scroll);
        let state = BarState {
            tool: None,
            style: self.style,
            can_undo: false,
            can_redo: false,
            mode: Mode::Scroll,
            can_scroll: false,
        };
        let mut output = match toolbar::show(ui, &painter, &bars, &state) {
            Some(BarAction::Output(output)) => Some(output),
            _ => None,
        };
        let (escape, copy, pin, save, ocr) = ui.input_mut(|i| {
            let copy_event = i
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Copy));
            (
                i.consume_key(Modifiers::NONE, Key::Escape),
                copy_event
                    || i.consume_key(Modifiers::COMMAND, Key::C)
                    || i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::COMMAND, Key::T),
                i.consume_key(Modifiers::COMMAND, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::E),
            )
        });
        if escape {
            output = Some(Output::Cancel);
        } else if copy {
            output = Some(Output::Copy);
        } else if pin {
            output = Some(Output::Pin);
        } else if save {
            output = Some(Output::Save);
        } else if ocr {
            output = Some(Output::Ocr);
        }
        if let Some(output) = output {
            return self.finish_scroll(output);
        }
        ctx.request_repaint_after(POLL);
        Outcome::Continue
    }

    /// 结束滚动截图，输出拼好的长图（还没截到画面时输出选区的截图）。
    fn finish_scroll(&mut self, output: Output) -> Outcome {
        let Some(selection) = self.selection else {
            return Outcome::Cancel;
        };
        let image = self.scroll.as_mut().and_then(ScrollCapture::finish);
        if output == Output::Cancel {
            return Outcome::Cancel;
        }
        let image = image.unwrap_or_else(|| self.capture.crop(selection));
        tracing::info!("滚动截图完成：{}×{}", image.width(), image.height());
        // 长图从选区的左上角开始往下延伸（贴图时由主界面缩小到屏幕放得下）。
        let rect = Rect::from_min_size(
            selection.min,
            vec2(image.width() as f32, image.height() as f32),
        );
        self.finish(output, image, rect)
    }

    /// 选区旁边的预览：长图缩小到预览的宽度，太长时显示最下面（最新拼上的）部分。
    fn paint_preview(&mut self, ctx: &Context, painter: &Painter, screen_rect: Rect, frame: Rect) {
        let Some(texture) = self
            .scroll
            .as_mut()
            .and_then(|scroll| scroll.preview_texture(ctx))
        else {
            return;
        };
        let height = frame.height().clamp(
            120.0_f32.min(screen_rect.height()),
            screen_rect.height() - 2.0 * PREVIEW_GAP,
        );
        let x = if frame.right() + PREVIEW_GAP + PREVIEW_WIDTH <= screen_rect.right() {
            frame.right() + PREVIEW_GAP
        } else if frame.left() - PREVIEW_GAP - PREVIEW_WIDTH >= screen_rect.left() {
            frame.left() - PREVIEW_GAP - PREVIEW_WIDTH
        } else {
            // 两边都放不下（选区几乎占满屏幕）：放在选区里面的左边（截取时会排除遮罩，不会截进去）。
            frame.left() + PREVIEW_GAP
        };
        let y = frame.top().clamp(
            screen_rect.top() + PREVIEW_GAP,
            screen_rect.bottom() - PREVIEW_GAP - height,
        );
        let outer = Rect::from_min_size(pos2(x, y), vec2(PREVIEW_WIDTH, height));
        // 按宽度缩放；比框高时只显示最下面的部分。
        let [tex_width, tex_height] = texture.size().map(|v| v as f32);
        let scale = PREVIEW_WIDTH / tex_width.max(1.0);
        let shown = (tex_height * scale).min(height);
        let image_rect = Rect::from_min_size(outer.min, vec2(PREVIEW_WIDTH, shown));
        let visible_rows = shown / scale;
        let uv = Rect::from_min_max(
            pos2(0.0, 1.0 - visible_rows / tex_height.max(1.0)),
            pos2(1.0, 1.0),
        );
        painter.rect_filled(image_rect.expand(3.0), 4.0, Color32::from_gray(250));
        painter.image(texture.id(), image_rect, uv, Color32::WHITE);
        painter.rect_stroke(
            image_rect.expand(3.0),
            4.0,
            Stroke::new(1.0, Color32::from_black_alpha(60)),
            StrokeKind::Inside,
        );
    }
}

/// 选区上方的提示。
fn status_text(status: Status, size: [u32; 2]) -> String {
    let [width, height] = size;
    match status {
        Status::Starting | Status::Ready => "长截图：向下滚动选区里的内容".to_owned(),
        Status::Scrolling => format!("长截图 {width} × {height} · 继续向下滚动，完成后按 ↩ 复制"),
        Status::Behind => format!("长截图 {width} × {height} · 已拼过这里，继续向下滚动"),
        Status::Lost => format!(
            "长截图 {width} × {height} · 跟丢了：滚得太快或滚到了开始位置之上，请滚回上次的位置"
        ),
        Status::Full => format!("长截图 {width} × {height} · 已达到最大长度"),
        Status::Failed => "长截图失败：请确认已允许 DevTools Hub 录屏".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn fit_frame_crops_and_resizes_to_selection() {
        let image = RgbaImage::from_fn(20, 10, |x, y| Rgba([x as u8, y as u8, 0, 255]));
        // 大小符合预期：直接裁出选区。
        let cropped = fit_frame(image.clone(), [20, 10], [3, 2], [10, 6]);
        assert_eq!(cropped.dimensions(), (10, 6));
        assert_eq!(cropped.get_pixel(0, 0), &Rgba([3, 2, 0, 255]));
        // 正好是选区：原样返回。
        assert!(fit_frame(image.clone(), [20, 10], [0, 0], [20, 10]) == image);
        // 大小不符：先缩放到预期大小，裁剪范围不越界。
        let resized = fit_frame(image, [40, 20], [35, 18], [10, 6]);
        assert_eq!(resized.dimensions(), (10, 6));
    }

    /// 在真实屏幕上检查截取范围的换算：选区的画面应与整屏截图里裁出的一样（边界故意落在奇数像素上）。
    #[test]
    #[ignore = "需要屏幕录制权限，手动运行"]
    fn platform_source_matches_display_capture() {
        let (_, display) = screen::cursor().expect("读不到鼠标位置");
        let image = screen::capture(&display).expect("截屏失败");
        let capture = Capture::new(display, image, &[]);
        let rect = Rect::from_min_size(pos2(101.0, 51.0), vec2(301.0, 201.0));
        let frame = platform_source(&capture, rect, None)().expect("截取选区失败");
        let expected = capture.crop(rect);
        assert_eq!(frame.dimensions(), expected.dimensions());
        // 两次截取之间屏幕可能有少量变化（时钟、光标闪烁），允许极少数像素不同。
        let differing = frame
            .pixels()
            .zip(expected.pixels())
            .filter(|(a, b)| a != b)
            .count();
        let total = (frame.width() * frame.height()) as usize;
        assert!(
            differing * 100 < total * 2,
            "{differing} / {total} 个像素不同，截取范围的换算可能错位了"
        );
    }

    #[test]
    fn status_text_mentions_size() {
        assert!(status_text(Status::Scrolling, [100, 2000]).contains("100 × 2000"));
        assert!(status_text(Status::Ready, [0, 0]).contains("向下滚动"));
    }
}
