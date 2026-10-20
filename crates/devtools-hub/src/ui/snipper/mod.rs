//! 截屏遮罩（参照 Snipaste 的 `Snipper`）。
//!
//! 先截下鼠标所在的整个显示器（`platform::screen`），再用一个盖住该显示器的无边框窗口显示这张静止的截图，
//! 在上面选择区域、标注——与 Snipaste 的日志一致：“Snipper: preparing… Background size… shown… ready”。
//!
//! - 移动鼠标时自动框出鼠标下的窗口，单击选中它（桌面上没有窗口时选中整个屏幕），拖动则框选任意区域；
//! - 选区可以拖动、拖八个控制点调整大小；方向键移动 1 像素，⇧+方向键调整宽高；
//! - 放大镜显示鼠标处的像素与颜色，按 C 复制颜色值，按 ⇧ 切换 RGB / HEX（选好区域后鼠标在选区内也显示）；
//! - 工具栏（`toolbar`）：矩形 / 椭圆、箭头、画笔、记号笔、马赛克、文字、橡皮擦，撤销重做，长截图，退出、贴图、保存、复制；
//! - 长截图（`scroll`）：选区内鼠标穿透，用户滚动下面的窗口，后台截取选区并拼成长图；
//! - 双击选区 / ↩ / ⌘C 复制，⌘T 贴图，⌘S 保存，⌘Z / ⇧⌘Z 撤销重做，右键先取消工具、再取消选区、最后退出，esc 退出。
//!
//! 编辑贴图（贴图右键菜单“编辑标注”或空格）：同样打开截屏遮罩，选区固定为贴图的范围、用贴图的原图作底，
//! 输出的图片写回这张贴图（与 Snipaste 贴图的标注工具条一样）。
//!
//! 输出：没有标注时直接从原图裁出选区，像素与屏幕完全一致；有标注时进入“导出”，只画截图和标注，
//! 用 `ViewportCommand::Screenshot` 截下这一帧再裁出选区，文字、抗锯齿都与屏幕上看到的一样。
//!
//! 截屏遮罩是 egui 的延迟视口（deferred viewport），与贴图一样有自己的窗口和重绘，只在截屏期间存在。
//! 不能用即时视口（immediate viewport）：即时视口的窗口在主界面的 `ui` 里同步创建，要用到 eframe
//! 线程局部的“当前事件循环”；面板隐藏时 eframe 从 `check_redraw_requests` 直接运行界面，那里没有设置它，
//! 窗口建不出来，egui 随即 panic（“the user callback was never called”）。延迟视口的窗口由 eframe
//! 在 `initialized_all_windows(event_loop)` 里创建，不受这个限制。

mod marks;
mod scroll;
#[cfg(test)]
mod tests;
mod toolbar;
mod watchdog;

pub use watchdog::take_rescued;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use egui::{
    pos2, vec2, Color32, ColorImage, CursorIcon, Event, FontId, Galley, Id, Key, LayerId,
    Modifiers, Order, Painter, PointerButton, Pos2, Rect, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, UserData, Vec2, ViewportBuilder, ViewportCommand, ViewportId, WindowLevel,
};
use image::RgbaImage;

use crate::platform::floating;
use crate::snip::annotation::{Mark, Marks, Mosaic, Style, Tool};
use crate::snip::{self, Capture, Handle};
use marks::TextDraft;
use scroll::{FrameFactory, ScrollCapture};
use toolbar::{BarAction, BarState, Mode};

/// 截屏遮罩窗口标题的前缀，后面跟着这次截屏的编号（平台层按标题找到窗口）。
const TITLE: &str = "DevTools Hub 截屏";

/// 下一次截屏的编号。每次截屏的遮罩都是新的视口（id、标题都不同），不会沿用上一次的窗口：
/// 上一次的视口可能还没被 eframe 清理（面板隐藏又没有贴图时 eframe 只调用 `logic`，不清理视口），
/// 沿用这个已经收起的窗口时，遮罩显示出来却不再绘制、不响应按键和鼠标，整个屏幕被盖住。
static NEXT_SNIP: AtomicU64 = AtomicU64::new(1);

/// 选区边框、控制点的颜色（与界面的强调色一致）。
const ACCENT: Color32 = Color32::from_rgb(0x30, 0x7b, 0xf6);
/// 选区外的遮罩：半透明的灰色，深色、浅色内容上都看得出来（纯黑的遮罩在深色界面上几乎看不见）。
const MASK: Color32 = Color32::from_rgba_premultiplied(43, 43, 43, 100);
/// 浮在截图上的文字底色（尺寸、放大镜信息）。
const BADGE: Color32 = Color32::from_rgba_premultiplied(24, 24, 26, 230);
/// 控制点的边长（点）。
const HANDLE_SIZE: f32 = 7.0;
/// 按下后移动超过这个距离（点）才算拖动，否则是单击。
const DRAG_THRESHOLD: f32 = 3.0;
/// 放大镜显示的像素数（奇数，中间一格是鼠标所在的像素）与每个像素放大后的边长（点）。
const MAGNIFIER_PIXELS: f32 = 15.0;
const MAGNIFIER_CELL: f32 = 8.0;
/// 马赛克格子的边长（点）。
const MOSAIC_BLOCK: f32 = 8.0;

/// 截屏结束时的结果。
pub enum Outcome {
    /// 还在截屏。
    Continue,
    Cancel,
    Copy(RgbaImage),
    Save(RgbaImage),
    /// 贴到屏幕上：`rect` 是选区在桌面上的位置（逻辑坐标），贴图出现在截屏的地方。
    Pin {
        image: RgbaImage,
        rect: Rect,
    },
    /// 复制放大镜下的颜色值（`#RRGGBB`）。
    CopyColor(String),
}

/// 输出方式（工具栏右侧的按钮与快捷键）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Cancel,
    Pin,
    Save,
    Copy,
}

/// 正在进行的拖动（像素坐标）。
#[derive(Clone, Copy, Debug)]
enum Drag {
    /// 按下了但还没移动：松开时算单击（选中鼠标下的窗口）。
    Pending { start: Pos2 },
    /// 框选新的区域。
    Create { start: Pos2 },
    /// 移动选区或拖动控制点。
    Adjust {
        handle: Handle,
        start: Pos2,
        rect: Rect,
    },
    /// 画标注（`draft`）。
    Draw { start: Pos2 },
    /// 用橡皮擦擦标注。
    Erase,
}

/// 正在导出带标注的图片：等截下的画面送回来。
struct Export {
    output: Output,
    /// 选区（像素）。
    rect: Rect,
    /// 已经发出截取画面的命令。
    requested: bool,
}

/// 正在编辑的贴图：它的图片与在截图中的位置（像素）。
struct Editing {
    image: Arc<RgbaImage>,
    texture: Option<TextureHandle>,
    rect: Rect,
}

/// 一次截屏的界面状态。
pub struct SnipSession {
    /// 这次截屏的编号（见 [`NEXT_SNIP`]）。
    number: u64,
    /// 遮罩窗口的标题。
    title: String,
    capture: Capture,
    texture: Option<TextureHandle>,
    /// 选区（像素坐标）。
    selection: Option<Rect>,
    drag: Option<Drag>,
    /// 最近一次知道的鼠标位置（像素坐标）。一开始是截屏时的位置：窗口刚出现、还没收到鼠标移动事件时
    /// 用它识别窗口、显示放大镜；鼠标移出窗口后也沿用它，松开鼠标时仍能结束拖动。
    pointer: Option<Pos2>,
    /// 遮罩窗口是否已经显示（Windows 上隐藏着创建，由主界面显示，见 `floating::OVERLAY_STARTS_HIDDEN`）。
    shown: bool,
    /// 平台层是否已经摆好窗口（层级、位置、焦点）。
    configured: bool,
    /// 摆好窗口后是否已经去掉显示前的遮挡（见 `floating::reveal_overlay`）。
    revealed: bool,
    /// 看门狗的看守编号（遮罩摆好后开始看守，见 [`watchdog`]）。
    watch: Option<u64>,
    /// 截屏结束时的结果，等主界面取走（见 [`Snipper::show`]）。
    outcome: Option<Outcome>,
    /// 截屏已经结束：之后只画静止的截图，等主界面收起窗口（结果可能已经被取走）。
    finished: bool,
    /// 选中的标注工具；没有选时拖动选区。
    tool: Option<Tool>,
    style: Style,
    marks: Marks,
    /// 正在画的标注。
    draft: Option<Mark>,
    /// 正在输入的文字。
    text: Option<TextDraft>,
    export: Option<Export>,
    /// 编辑贴图时不为空：选区固定，不能调整。
    editing: Option<Editing>,
    /// 放大镜显示、按 C 复制的颜色格式：true 是 `rgb(R, G, B)`，默认 `#RRGGBB`。
    rgb: bool,
    /// 上一帧 ⇧ 是否按着。
    shift_down: bool,
    /// ⇧ 按下后还没和别的键、鼠标一起用：松开时算单独按了一下 ⇧（切换颜色格式）。
    shift_tap: bool,
    /// 正在滚动截图（长截图）。
    scroll: Option<ScrollCapture>,
    /// 滚动截图截取选区的方式（测试里换成模拟的画面）。
    frame_source: FrameFactory,
}

/// 截屏遮罩窗口：持有截屏状态，由主界面每帧调用 [`Snipper::show`] 维持。
pub struct Snipper {
    session: Arc<Mutex<SnipSession>>,
    viewport: ViewportId,
    /// 遮罩窗口的标题。
    title: String,
    /// 编辑的贴图编号（编辑贴图时）。
    pin: Option<u64>,
}

impl Snipper {
    /// `cursor` 是截屏时鼠标的全局坐标（平台单位）。
    pub fn new(capture: Capture, cursor: [f64; 2]) -> Self {
        Self::from_session(SnipSession::new(capture, cursor), None)
    }

    /// 编辑贴图 `pin`：`image` 是贴图的图片，`rect` 是贴图窗口在桌面上的位置（逻辑坐标）。
    pub fn edit_pin(
        capture: Capture,
        cursor: [f64; 2],
        pin: u64,
        image: Arc<RgbaImage>,
        rect: Rect,
    ) -> Self {
        let mut session = SnipSession::new(capture, cursor);
        session.edit(image, rect);
        Self::from_session(session, Some(pin))
    }

    fn from_session(session: SnipSession, pin: Option<u64>) -> Self {
        Self {
            viewport: session.viewport_id(),
            title: session.title.clone(),
            session: Arc::new(Mutex::new(session)),
            pin,
        }
    }

    /// 遮罩窗口的标题（平台层按它找到窗口）。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// 遮罩窗口还没显示出来（隐藏着创建、等主界面显示）。这期间主界面要一直运行（见 `panel::wake`）。
    pub fn waiting_to_show(&self) -> bool {
        !self.session.lock().unwrap().shown
    }

    /// 编辑的贴图编号；普通截屏时为空。
    pub fn pin(&self) -> Option<u64> {
        self.pin
    }

    /// 是否画了标注（编辑贴图时，有标注才需要更新贴图的图片）。
    pub fn has_marks(&self) -> bool {
        !self.session.lock().unwrap().marks.is_empty()
    }

    /// 维持截屏遮罩窗口，返回截屏结果（还在截屏时是 `Continue`）。必须在主界面的 `ui` 里每帧调用，
    /// 直到返回的不是 `Continue`；之后不再调用，egui 在下一帧结束时关闭窗口。
    ///
    /// 结果是保存时例外：保存对话框要显示在遮罩上面，遮罩立即收起、不再需要，这一帧就不维持，
    /// egui 在这一帧结束时关闭窗口。
    ///
    /// 其余结果的这一帧也要维持窗口：哪一帧没有调用 `show_viewport_deferred`，egui 就在这一帧结束时销毁窗口，
    /// 之后再调用（截屏结束后遮罩还要停留一会儿，见 `closing_snip_ui`）会新建一个窗口。新窗口没有经过
    /// `floating::begin_overlay`，被系统限制在菜单栏和程序坞之间，画出来的截图被压扁、错开，整屏闪一下。
    pub fn show(&self, ctx: &egui::Context) -> Outcome {
        let (builder, outcome) = {
            let mut session = self.session.lock().unwrap();
            // 导出的画面可能被送到了主界面这一帧的输入里（eframe 把截图事件交给下一个运行的视口）。
            if let Some(image) = ctx.input(|i| screenshot_event(&i.events, self.viewport)) {
                let outcome = session.receive_screenshot(&image);
                if !matches!(outcome, Outcome::Continue) {
                    session.outcome = Some(outcome);
                    session.finished = true;
                }
            }
            let outcome = session.outcome.take().unwrap_or(Outcome::Continue);
            if !session.configured {
                // 窗口在下一次运行界面时由 eframe 创建，创建后还要画一帧才会显示、摆好位置：
                // 在那之前主界面和遮罩都要继续重绘（面板隐藏时没有别的事件会触发它们）。
                ctx.request_repaint();
                ctx.request_repaint_of(self.viewport);
                if !session.shown {
                    // 隐藏着的窗口 eframe 不运行它的界面，由主界面把它显示出来。
                    session.shown =
                        floating::show_overlay(&session.title, session.capture.display.frame);
                }
            }
            (session.builder(), outcome)
        };
        if matches!(outcome, Outcome::Save(_)) {
            return outcome;
        }
        let session = self.session.clone();
        ctx.show_viewport_deferred(self.viewport, builder, move |ui, _| {
            watchdog::frame_painted();
            let mut session = session.lock().unwrap();
            if session.finished {
                // 已经结束、等主界面收起窗口：只画静止的截图（没有遮罩和工具栏），看起来与真实屏幕一样。
                session.paint_still(ui);
                return;
            }
            let outcome = session.ui(ui);
            if !matches!(outcome, Outcome::Continue) {
                session.outcome = Some(outcome);
                session.finished = true;
                ui.ctx().request_repaint_of(ViewportId::ROOT);
            }
        });
        outcome
    }
}

impl Drop for Snipper {
    /// 截屏结束、遮罩收起后主界面丢掉 `Snipper`：停止看守。
    fn drop(&mut self) {
        if let Some(id) = self
            .session
            .lock()
            .ok()
            .and_then(|mut session| session.watch.take())
        {
            watchdog::stop(id);
        }
    }
}

impl SnipSession {
    /// `cursor` 是截屏时鼠标的全局坐标（平台单位）。
    pub fn new(capture: Capture, cursor: [f64; 2]) -> Self {
        let initial = capture.global_to_pixels(cursor);
        let number = NEXT_SNIP.fetch_add(1, Ordering::Relaxed);
        Self {
            number,
            title: format!("{TITLE} {number}"),
            pointer: capture.bounds().contains(initial).then_some(initial),
            capture,
            texture: None,
            selection: None,
            drag: None,
            shown: !floating::OVERLAY_STARTS_HIDDEN,
            configured: false,
            revealed: false,
            watch: None,
            outcome: None,
            finished: false,
            tool: None,
            style: Style::default(),
            marks: Marks::default(),
            draft: None,
            text: None,
            export: None,
            editing: None,
            rgb: false,
            shift_down: false,
            shift_tap: false,
            scroll: None,
            frame_source: scroll::platform_source,
        }
    }

    /// 进入编辑贴图：选区固定为贴图的范围（`rect` 是逻辑坐标），底图换成贴图的原图。
    fn edit(&mut self, image: Arc<RgbaImage>, rect: Rect) {
        let rect = self.capture.pixel_rect(rect);
        self.selection = Some(rect);
        self.editing = Some(Editing {
            image,
            texture: None,
            rect,
        });
    }

    /// 这次截屏的遮罩的视口 id。
    fn viewport_id(&self) -> ViewportId {
        ViewportId::from_hash_of(("devtools-hub-snipper", self.number))
    }

    /// 遮罩窗口的参数：无边框、置顶，铺满截屏的显示器（平台层随后按原生坐标摆正）。
    fn builder(&self) -> ViewportBuilder {
        let logical = self.capture.display.logical;
        ViewportBuilder::default()
            .with_title(self.title.clone())
            .with_decorations(false)
            .with_position(logical.min)
            .with_inner_size(logical.size())
            .with_resizable(false)
            .with_has_shadow(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(!floating::OVERLAY_STARTS_HIDDEN)
            // 窗口刚创建、还没画出第一帧时，macOS 显示的是窗口的背景色（灰色），全屏的遮罩就会让整个屏幕灰一下。
            // 透明窗口在这一刻什么都不显示，第一帧画上截图后看起来与真实屏幕一样。
            // Windows 上 wgpu 的透明窗口支持不稳定（面板也没有用），保持不透明。
            .with_transparent(cfg!(target_os = "macos"))
    }

    fn ui(&mut self, ui: &mut Ui) -> Outcome {
        let ctx = ui.ctx().clone();
        if self.configured && !self.revealed {
            // 上一帧已经按铺满屏幕的尺寸画好了，可以露出来。
            floating::reveal_overlay(&self.title);
            self.revealed = true;
        }
        if !self.configured {
            self.configured = floating::begin_overlay(&self.title, self.capture.display.frame);
            if self.configured {
                self.watch = Some(watchdog::start(ctx.clone(), self.capture.display.frame));
            }
            ctx.request_repaint();
        }
        if self.scroll.is_some() {
            return self.scroll_ui(ui, &ctx);
        }
        // 整个窗口（不受外层布局的边距影响，测试里 kittest 会给界面加边距）。
        let screen = ctx.viewport_rect();
        let view = View::new(screen, self.capture.bounds());
        if let Some(latest) = ui.input(|i| i.pointer.latest_pos()) {
            self.pointer = Some(view.to_pixels(latest));
        }
        let pointer = self.pointer;
        let texture = self.texture(&ctx);
        let painter = ui.painter().with_clip_rect(screen);
        self.paint_content(&painter, &texture, &view);
        if self.export.is_some() {
            return self.exporting(ui, &ctx);
        }

        // 工具栏：选好区域后显示（框选、调整选区时不显示）。先算出位置，鼠标在工具栏上时不参与选区和标注操作。
        let mode = if self.editing.is_some() {
            Mode::Edit
        } else {
            Mode::Snip
        };
        let bars = match (self.selection, self.drag) {
            (Some(selection), None | Some(Drag::Draw { .. }) | Some(Drag::Erase)) => Some(
                toolbar::layout(view.to_points(selection), screen, self.tool, mode),
            ),
            _ => None,
        };
        let latest = ui.input(|i| i.pointer.latest_pos());
        let over_bar = bars.zip(latest).is_some_and(|(bars, p)| bars.contains(p));
        let over_text = self
            .text
            .as_ref()
            .and_then(|text| text.rect)
            .zip(latest)
            .is_some_and(|(rect, p)| rect.contains(p));

        let pick_color = self.shows_magnifier(pointer, over_bar);
        if self.shift_tapped(ui) && pick_color {
            self.rgb = !self.rgb;
        }
        let mut outcome = if self.text.is_some() {
            self.text_keys(ui, &ctx, &view);
            Outcome::Continue
        } else {
            self.keyboard(ui, &ctx, &view, pointer, pick_color)
        };
        if matches!(outcome, Outcome::Continue) && !over_bar && !over_text {
            outcome = self.pointer(ui, &ctx, &view, pointer);
        }

        // 高亮区域：选区；没有选区时是鼠标下的窗口（没有窗口时是整个屏幕）。
        let highlight = self.selection.or_else(|| match self.drag {
            None | Some(Drag::Pending { .. }) => Some(self.target(pointer)),
            _ => None,
        });
        if let Some(highlight) = highlight {
            let hole = view.to_points(highlight);
            // 编辑贴图时不加遮罩（与 Snipaste 一样只在贴图旁出现工具条），屏幕看起来不变。
            if self.editing.is_none() {
                paint_mask(&painter, screen, hole);
            }
            painter.rect_stroke(hole, 0.0, Stroke::new(1.5, ACCENT), StrokeKind::Outside);
            if let Some(selection) = self.selection.filter(|_| self.editing.is_none()) {
                paint_handles(&painter, view.to_points(selection));
                size_badge(&painter, screen, hole, selection.size());
            }
        } else {
            painter.rect_filled(screen, 0.0, MASK);
        }

        let text_area = self
            .selection
            .map_or(screen, |selection| view.to_points(selection));
        if let Some(text) = &mut self.text {
            text.show(&ctx, &view, text_area);
        }
        if let Some(bars) = bars {
            let state = BarState {
                tool: self.tool,
                style: self.style,
                can_undo: self.marks.can_undo(),
                can_redo: self.marks.can_redo(),
                mode,
                can_scroll: self.can_scroll(),
            };
            if let Some(action) = toolbar::show(ui, &painter, &bars, &state) {
                if let Some(result) = self.apply_bar(&ctx, &view, action) {
                    outcome = result;
                }
            }
        }

        self.update_cursor(&ctx, &view, pointer, over_bar, over_text);

        if let Some(pointer) = pointer.filter(|_| self.shows_magnifier(pointer, over_bar)) {
            let layer = LayerId::new(Order::Foreground, Id::new("snip-magnifier"));
            self.magnifier(&ctx.layer_painter(layer), &texture, &view, pointer);
        }
        outcome
    }

    /// 是否显示放大镜（这时按 C 复制鼠标处的颜色）：选区域、拖动控制点时显示；选好区域后，
    /// 没选标注工具时鼠标在选区内也显示（选了标注工具、移动选区、输入文字、鼠标在工具栏上时不显示）。
    fn shows_magnifier(&self, pointer: Option<Pos2>, over_bar: bool) -> bool {
        let Some(pointer) = pointer.filter(|p| self.capture.bounds().contains(*p)) else {
            return false;
        };
        match self.drag {
            Some(Drag::Create { .. }) => true,
            Some(Drag::Adjust { handle, .. }) => handle != Handle::Move,
            Some(Drag::Draw { .. } | Drag::Erase) => false,
            _ => match self.selection {
                None => true,
                Some(selection) => {
                    self.tool.is_none()
                        && self.text.is_none()
                        && !over_bar
                        && selection.contains(pointer)
                }
            },
        }
    }

    /// 截图与选区里的标注（导出时只画这些）。
    fn paint_content(&mut self, painter: &Painter, texture: &TextureHandle, view: &View) {
        painter.image(
            texture.id(),
            view.screen,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        if let Some(editing) = &mut self.editing {
            // 贴图的原图（不是截图里显示的那份：那里可能缩放过、带着悬停时的边框）。
            let texture = editing.texture.get_or_insert_with(|| {
                let max = painter.ctx().input(|i| i.max_texture_side);
                painter.ctx().load_texture(
                    "snip-edit-pin",
                    snip::texture_image(&editing.image, max),
                    TextureOptions::LINEAR,
                )
            });
            let rect = view.to_points(editing.rect);
            if snip::has_alpha(&editing.image) {
                painter.rect_filled(rect, 0.0, Color32::WHITE);
            }
            painter.image(
                texture.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if let Some(selection) = self.selection {
            // 标注只画在选区里（与 Snipaste 默认的“不在选区外绘制”一致）。
            let clipped = painter.with_clip_rect(view.to_points(selection).intersect(view.screen));
            for mark in self.marks.items().iter().chain(&self.draft) {
                marks::paint(&clipped, view, mark);
            }
        }
    }

    /// 截屏已经结束、窗口还没关闭时：只画截图和标注，看起来与真实的屏幕一样。
    fn paint_still(&mut self, ui: &Ui) {
        if self.scroll.is_some() {
            // 滚动截图时窗口里本来就没有截图（看到的是真实的屏幕），什么都不画。
            return;
        }
        let ctx = ui.ctx().clone();
        let screen = ctx.viewport_rect();
        let view = View::new(screen, self.capture.bounds());
        let texture = self.texture(&ctx);
        self.paint_content(&ui.painter().with_clip_rect(screen), &texture, &view);
    }

    /// 导出中：画面上只有截图和标注，请求截下这一帧，等画面送回来后裁出选区。
    fn exporting(&mut self, ui: &Ui, ctx: &egui::Context) -> Outcome {
        if let Some(export) = &mut self.export {
            if !export.requested {
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
                export.requested = true;
            }
        }
        let viewport = self.viewport_id();
        if let Some(image) = ui.input(|i| screenshot_event(&i.events, viewport)) {
            return self.receive_screenshot(&image);
        }
        ctx.set_cursor_icon(CursorIcon::Wait);
        ctx.request_repaint();
        Outcome::Continue
    }

    /// 截下的画面送回来了：裁出选区，完成导出。没有在导出时忽略。
    fn receive_screenshot(&mut self, image: &ColorImage) -> Outcome {
        let Some(export) = self.export.take() else {
            return Outcome::Continue;
        };
        // 截下的画面是窗口的物理像素，一般与截图一样大；不一样（例如截图被缩小显示）时按比例换算选区。
        let bounds = self.capture.bounds();
        let k = vec2(
            image.size[0] as f32 / bounds.width(),
            image.size[1] as f32 / bounds.height(),
        );
        let rect = Rect::from_min_max(
            (export.rect.min.to_vec2() * k).to_pos2(),
            (export.rect.max.to_vec2() * k).to_pos2(),
        )
        .intersect(Rect::from_min_size(
            Pos2::ZERO,
            vec2(image.size[0] as f32, image.size[1] as f32),
        ));
        let (x0, y0) = (rect.min.x.round() as usize, rect.min.y.round() as usize);
        let (width, height) = (
            (rect.width().round() as usize).max(1),
            (rect.height().round() as usize).max(1),
        );
        let mut cropped = RgbaImage::new(width as u32, height as u32);
        for (x, y, pixel) in cropped.enumerate_pixels_mut() {
            let (sx, sy) = (
                (x0 + x as usize).min(image.size[0] - 1),
                (y0 + y as usize).min(image.size[1] - 1),
            );
            let [r, g, b, _] = image.pixels[sy * image.size[0] + sx].to_array();
            *pixel = image::Rgba([r, g, b, 255]);
        }
        self.finish(export.output, cropped, export.rect)
    }

    /// 截图的纹理（第一帧上传）。超过显卡支持的最大尺寸时缩小显示，裁剪仍然用原图。
    fn texture(&mut self, ctx: &egui::Context) -> TextureHandle {
        self.texture
            .get_or_insert_with(|| {
                let max = ctx.input(|i| i.max_texture_side);
                ctx.load_texture(
                    "snip-screen",
                    snip::texture_image(&self.capture.image, max),
                    TextureOptions::NEAREST,
                )
            })
            .clone()
    }

    /// 没有选区时输出的区域：鼠标下的窗口，没有窗口时是整个屏幕。
    fn target(&self, pointer: Option<Pos2>) -> Rect {
        pointer
            .and_then(|p| self.capture.window_at(p))
            .unwrap_or_else(|| self.capture.bounds())
    }

    /// 处理按键，返回结果（没有结束截屏时是 `Continue`）。正在输入文字时不调用（见 [`Self::text_keys`]）。
    fn keyboard(
        &mut self,
        ui: &Ui,
        ctx: &egui::Context,
        view: &View,
        pointer: Option<Pos2>,
        pick_color: bool,
    ) -> Outcome {
        let bounds = self.capture.bounds();
        let (escape, enter, copy, color, pin, save, redo, undo, arrows) = ui.input_mut(|i| {
            let copy_event = i.events.iter().any(|event| matches!(event, Event::Copy));
            let arrows = [
                (Key::ArrowLeft, vec2(-1.0, 0.0)),
                (Key::ArrowRight, vec2(1.0, 0.0)),
                (Key::ArrowUp, vec2(0.0, -1.0)),
                (Key::ArrowDown, vec2(0.0, 1.0)),
            ]
            .into_iter()
            .filter_map(|(key, delta)| {
                if i.consume_key(Modifiers::SHIFT, key) {
                    Some((delta, true))
                } else if i.consume_key(Modifiers::NONE, key) {
                    Some((delta, false))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
            let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y);
            (
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::Enter),
                copy_event || i.consume_key(Modifiers::COMMAND, Key::C),
                i.consume_key(Modifiers::NONE, Key::C),
                i.consume_key(Modifiers::COMMAND, Key::T),
                i.consume_key(Modifiers::COMMAND, Key::S),
                redo,
                i.consume_key(Modifiers::COMMAND, Key::Z),
                arrows,
            )
        });
        if escape {
            return Outcome::Cancel;
        }
        if undo {
            self.marks.undo();
        }
        if redo {
            self.marks.redo();
        }
        if let Some(selection) = self.selection.filter(|_| self.editing.is_none()) {
            for (delta, resize) in arrows {
                self.selection = Some(snip::nudge(selection, delta, resize, bounds));
            }
        }
        if enter || copy {
            return self.output(ctx, view, Output::Copy, pointer);
        }
        if pin {
            return self.output(ctx, view, Output::Pin, pointer);
        }
        if save {
            return self.output(ctx, view, Output::Save, pointer);
        }
        if color && pick_color {
            if let Some(rgba) = pointer.and_then(|p| self.capture.pixel(p)) {
                return Outcome::CopyColor(self.color_text(rgba));
            }
        }
        Outcome::Continue
    }

    /// 这一帧是否单独按了一下 ⇧（松开时判断：按着 ⇧ 期间按了别的键或鼠标就不算，例如 ⇧+方向键）。
    fn shift_tapped(&mut self, ui: &Ui) -> bool {
        // 按事件顺序处理：一帧里可能同时有 ⇧ 的按下和松开（按得快、或者事件攒在一起送来）。
        let mut tapped = false;
        ui.input(|i| {
            for event in &i.events {
                match event {
                    Event::ModifiersChanged(modifiers) => {
                        tapped |= self.shift_changed(modifiers.shift);
                    }
                    Event::Key { pressed: true, .. }
                    | Event::PointerButton { pressed: true, .. } => {
                        self.shift_tap = false;
                    }
                    _ => {}
                }
            }
            tapped |= self.shift_changed(i.modifiers.shift);
        });
        tapped
    }

    /// 记下 ⇧ 现在的状态，返回这次是否是单独按了一下 ⇧ 后松开。
    fn shift_changed(&mut self, shift: bool) -> bool {
        if shift && !self.shift_down {
            self.shift_tap = true;
        }
        let tapped = !shift && self.shift_down && self.shift_tap;
        self.shift_down = shift;
        tapped
    }

    /// 按当前的颜色格式写出颜色值（放大镜显示、按 C 复制的都是它）。
    fn color_text(&self, rgba: [u8; 4]) -> String {
        if self.rgb {
            snip::rgb_color(rgba)
        } else {
            snip::hex_color(rgba)
        }
    }

    /// 输入文字时的按键：esc 或 ⌘↩ 结束输入，其余交给输入框。
    fn text_keys(&mut self, ui: &Ui, ctx: &egui::Context, view: &View) {
        let done = ui.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::Escape)
                | i.consume_key(Modifiers::COMMAND, Key::Enter)
        });
        if done {
            self.commit_text(ctx, view);
        }
    }

    /// 处理鼠标：单击选中窗口、拖动框选、调整选区、画标注、双击复制、右键取消。
    fn pointer(
        &mut self,
        ui: &Ui,
        ctx: &egui::Context,
        view: &View,
        pointer: Option<Pos2>,
    ) -> Outcome {
        let bounds = self.capture.bounds();
        let (pressed, down, released, double, secondary) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.button_double_clicked(PointerButton::Primary),
                i.pointer.button_clicked(PointerButton::Secondary),
            )
        });
        if secondary {
            // 右键逐级后退：结束输入文字 → 取消工具 → 取消选区（连同标注）→ 退出截屏。
            if self.text.is_some() {
                self.commit_text(ctx, view);
            } else if self.tool.is_some() {
                self.tool = None;
                self.draft = None;
                self.drag = None;
            } else if self.selection.is_some() && self.editing.is_none() {
                self.selection = None;
                self.drag = None;
                self.draft = None;
                self.marks = Marks::default();
            } else {
                return Outcome::Cancel;
            }
            return Outcome::Continue;
        }
        let Some(pointer) = pointer else {
            return Outcome::Continue;
        };
        if double && self.tool.is_none() && self.selection.is_some_and(|s| s.contains(pointer)) {
            self.drag = None;
            return self.output(ctx, view, Output::Copy, Some(pointer));
        }
        if pressed {
            if self.text.is_some() {
                // 点在输入框外面：结束输入（点在输入框里时不会走到这里）。
                self.commit_text(ctx, view);
            }
            self.drag = match self.selection {
                Some(selection) => {
                    // 编辑贴图时选区固定，只能画标注。
                    let hit = snip::hit(selection, pointer, HANDLE_SIZE * view.scale)
                        .filter(|_| self.editing.is_none());
                    match (self.tool, hit) {
                        // 控制点总能调整选区大小，选了标注工具也一样。
                        (_, Some(handle)) if handle != Handle::Move => Some(Drag::Adjust {
                            handle,
                            start: pointer,
                            rect: selection,
                        }),
                        (Some(tool), _) if selection.contains(pointer) => {
                            self.begin_mark(tool, pointer, view)
                        }
                        (None, Some(Handle::Move)) => Some(Drag::Adjust {
                            handle: Handle::Move,
                            start: pointer,
                            rect: selection,
                        }),
                        _ => None,
                    }
                }
                None => Some(Drag::Pending { start: pointer }),
            };
        }
        if down {
            match self.drag {
                Some(Drag::Pending { start })
                    if (pointer - start).length() > DRAG_THRESHOLD * view.scale =>
                {
                    self.drag = Some(Drag::Create { start });
                    self.selection = Some(snip::rect_between(start, pointer, bounds));
                }
                Some(Drag::Create { start }) => {
                    self.selection = Some(snip::rect_between(start, pointer, bounds));
                }
                Some(Drag::Adjust {
                    handle,
                    start,
                    rect,
                }) => {
                    self.selection = Some(snip::drag_handle(rect, handle, pointer - start, bounds));
                }
                Some(Drag::Draw { start }) => self.update_mark(start, pointer),
                Some(Drag::Erase) => {
                    let reach = self.eraser_reach(view);
                    self.marks.erase_at(pointer, reach);
                }
                _ => {}
            }
        }
        if released {
            match self.drag.take() {
                Some(Drag::Pending { .. }) => self.selection = Some(self.target(Some(pointer))),
                Some(Drag::Draw { .. }) => self.commit_mark(),
                _ => {}
            }
        }
        Outcome::Continue
    }

    /// 在 `point` 开始画一个标注；返回接下来的拖动（文字工具没有拖动，直接出现输入框）。
    fn begin_mark(&mut self, tool: Tool, point: Pos2, view: &View) -> Option<Drag> {
        let width = self.style.size.points(tool) * view.scale;
        let color = self.style.color;
        self.draft = Some(match tool {
            Tool::Shape => Mark::Shape {
                rect: Rect::from_two_pos(point, point),
                ellipse: self.style.ellipse,
                color,
                width,
            },
            Tool::Arrow => Mark::Arrow {
                from: point,
                to: point,
                color,
                width,
            },
            Tool::Pen => Mark::Stroke {
                points: vec![point],
                color,
                width,
            },
            Tool::Marker => Mark::Stroke {
                points: vec![point],
                color: self.style.marker_color(),
                width,
            },
            Tool::Mosaic => {
                let block = (MOSAIC_BLOCK * view.scale).round();
                Mark::Mosaic(Mosaic::new(&self.capture.image, point, width, block))
            }
            Tool::Text => {
                self.text = Some(TextDraft::new(point, color, width));
                return None;
            }
            Tool::Eraser => {
                self.marks.erase_at(point, self.eraser_reach(view));
                return Some(Drag::Erase);
            }
        });
        Some(Drag::Draw { start: point })
    }

    /// 拖动中更新正在画的标注（限制在选区里）。
    fn update_mark(&mut self, start: Pos2, point: Pos2) {
        let Some(selection) = self.selection else {
            return;
        };
        let point = point.clamp(selection.min, selection.max);
        let image = &self.capture.image;
        match &mut self.draft {
            Some(Mark::Shape { rect, .. }) => *rect = Rect::from_two_pos(start, point),
            Some(Mark::Arrow { to, .. }) => *to = point,
            Some(Mark::Stroke { points, .. })
                if points
                    .last()
                    .is_none_or(|last| (point - *last).length() >= 1.0) =>
            {
                points.push(point);
            }
            Some(Mark::Mosaic(mosaic))
                if mosaic
                    .points
                    .last()
                    .is_none_or(|last| (point - *last).length() >= 1.0) =>
            {
                mosaic.extend(image, point);
            }
            _ => {}
        }
    }

    /// 松开鼠标：保留画好的标注（太小的矩形、太短的箭头当作误点丢掉）。
    fn commit_mark(&mut self) {
        let Some(mark) = self.draft.take() else {
            return;
        };
        let keep = match &mark {
            Mark::Shape { rect, .. } => rect.width() >= 2.0 && rect.height() >= 2.0,
            Mark::Arrow { from, to, .. } => (*to - *from).length() >= 4.0,
            _ => true,
        };
        if keep {
            self.marks.push(mark);
        }
    }

    /// 结束输入文字：有内容时变成一个标注。
    fn commit_text(&mut self, ctx: &egui::Context, view: &View) {
        if let Some(text) = self.text.take() {
            let painter = ctx.layer_painter(LayerId::background());
            let area = self
                .selection
                .map_or(view.screen, |selection| view.to_points(selection));
            if let Some(mark) = text.finish(&painter, view, area) {
                self.marks.push(mark);
            }
        }
    }

    /// 橡皮擦的判定距离（像素）。
    fn eraser_reach(&self, view: &View) -> f32 {
        self.style.size.points(Tool::Eraser) * view.scale / 2.0
    }

    /// 执行工具栏上的操作，输出时返回结果。
    fn apply_bar(
        &mut self,
        ctx: &egui::Context,
        view: &View,
        action: BarAction,
    ) -> Option<Outcome> {
        match action {
            BarAction::Tool(tool) => {
                self.commit_text(ctx, view);
                // 再点一次已选的工具就取消选择，回到调整选区。
                self.tool = (self.tool != Some(tool)).then_some(tool);
            }
            BarAction::Undo => {
                self.commit_text(ctx, view);
                self.marks.undo();
            }
            BarAction::Redo => {
                self.commit_text(ctx, view);
                self.marks.redo();
            }
            BarAction::Scroll => self.start_scroll(ctx, view),
            BarAction::Output(output) => {
                let pointer = self.pointer;
                return Some(self.output(ctx, view, output, pointer));
            }
            BarAction::Size(size) => {
                self.style.size = size;
                if let Some(text) = &mut self.text {
                    text.size = size.points(Tool::Text) * view.scale;
                }
            }
            BarAction::Ellipse(ellipse) => self.style.ellipse = ellipse,
            BarAction::Color(color) => {
                self.style.color = color;
                if let Some(text) = &mut self.text {
                    text.color = color;
                }
            }
        }
        None
    }

    /// 按工具栏按钮或快捷键输出：没有选区时输出鼠标下的窗口（或整个屏幕）。
    /// 有标注时先导出（截下带标注的画面），结果稍后由 [`Self::receive_screenshot`] 给出。
    fn output(
        &mut self,
        ctx: &egui::Context,
        view: &View,
        output: Output,
        pointer: Option<Pos2>,
    ) -> Outcome {
        if output == Output::Cancel {
            return Outcome::Cancel;
        }
        self.commit_text(ctx, view);
        self.commit_mark();
        let rect = self.selection.unwrap_or_else(|| self.target(pointer));
        if let Some(editing) = self.editing.as_ref().filter(|_| self.marks.is_empty()) {
            // 编辑贴图却没有画标注：输出贴图的原图，分辨率不变。
            let image = (*editing.image).clone();
            return self.finish(output, image, rect);
        }
        if self.selection.is_some() && !self.marks.is_empty() {
            self.export = Some(Export {
                output,
                rect,
                requested: false,
            });
            ctx.request_repaint();
            return Outcome::Continue;
        }
        let image = self.capture.crop(rect);
        self.finish(output, image, rect)
    }

    /// 输出裁好的图片。
    fn finish(&self, output: Output, image: RgbaImage, rect: Rect) -> Outcome {
        match output {
            Output::Cancel => Outcome::Cancel,
            Output::Copy => Outcome::Copy(image),
            Output::Save => Outcome::Save(image),
            Output::Pin => Outcome::Pin {
                image,
                rect: self.capture.logical_rect(rect),
            },
        }
    }

    /// 鼠标指针：工具栏上是手形，控制点上是调整大小的箭头；画标注时是十字（文字是 I 形），
    /// 调整选区时选区内是移动，没有选区时是十字。
    fn update_cursor(
        &self,
        ctx: &egui::Context,
        view: &View,
        pointer: Option<Pos2>,
        over_bar: bool,
        over_text: bool,
    ) {
        let handle = self
            .selection
            .zip(pointer)
            .filter(|_| self.editing.is_none())
            .and_then(|(selection, p)| snip::hit(selection, p, HANDLE_SIZE * view.scale));
        let inside = self
            .selection
            .zip(pointer)
            .is_some_and(|(selection, p)| selection.contains(p));
        let icon = if over_bar {
            CursorIcon::PointingHand
        } else if over_text {
            CursorIcon::Text
        } else if let Some(Drag::Adjust { handle, .. }) = self.drag {
            handle.cursor()
        } else if let Some(handle) = handle.filter(|handle| *handle != Handle::Move) {
            handle.cursor()
        } else if let Some(tool) = self.tool {
            match tool {
                _ if !inside => CursorIcon::Default,
                Tool::Text => CursorIcon::Text,
                _ => CursorIcon::Crosshair,
            }
        } else if self.selection.is_some() {
            if inside && self.editing.is_none() {
                CursorIcon::Move
            } else {
                CursorIcon::Default
            }
        } else {
            CursorIcon::Crosshair
        };
        ctx.set_cursor_icon(icon);
    }

    /// 放大镜：鼠标右下方显示周围 15×15 像素的放大图，下面居中写坐标、颜色值和按键提示。
    fn magnifier(&self, painter: &Painter, texture: &TextureHandle, view: &View, pointer: Pos2) {
        let center = pos2(pointer.x.floor(), pointer.y.floor());
        let half = (MAGNIFIER_PIXELS - 1.0) / 2.0;
        let source = Rect::from_min_size(center - Vec2::splat(half), Vec2::splat(MAGNIFIER_PIXELS));
        let side = MAGNIFIER_PIXELS * MAGNIFIER_CELL;

        // 先排好下面的文字：一行一句，宽度超过放大图时整个放大镜跟着变宽。
        let font = FontId::proportional(11.5);
        let text_color = Color32::from_gray(235);
        let hint_color = Color32::from_gray(170);
        let line = |text: String, color: Color32| painter.layout_no_wrap(text, font.clone(), color);
        let position = line(
            format!("({}, {})", center.x as i32, center.y as i32),
            text_color,
        );
        let color = self
            .capture
            .pixel(center)
            .map(|rgba| (rgba, line(self.color_text(rgba), text_color)));
        let hints = [
            line("按 C 复制颜色值".to_owned(), hint_color),
            line("按 Shift 切换 RGB/HEX".to_owned(), hint_color),
        ];
        const SWATCH: f32 = 12.0;
        const SWATCH_GAP: f32 = 6.0;
        const LINE: f32 = 17.0;
        const PADDING: f32 = 8.0;
        let color_width = color
            .as_ref()
            .map_or(0.0, |(_, galley)| SWATCH + SWATCH_GAP + galley.size().x);
        let text_width = hints
            .iter()
            .map(|galley| galley.size().x)
            .fold(position.size().x.max(color_width), f32::max);
        let lines = if color.is_some() { 4.0 } else { 3.0 };
        let size = vec2(
            side.max(text_width + PADDING * 2.0),
            side + 6.0 + lines * LINE + 4.0,
        );

        let cursor = view.to_point(pointer);
        let screen = view.screen;
        let x = if cursor.x + 20.0 + size.x <= screen.right() {
            cursor.x + 20.0
        } else {
            cursor.x - 20.0 - size.x
        };
        let y = if cursor.y + 20.0 + size.y <= screen.bottom() {
            cursor.y + 20.0
        } else {
            cursor.y - 20.0 - size.y
        };
        let frame = Rect::from_min_size(pos2(x, y), size);
        let zoom = Rect::from_min_size(
            pos2(frame.center().x - side / 2.0, frame.top()),
            Vec2::splat(side),
        );
        painter.rect_filled(frame.expand(1.0), 4.0, BADGE);
        painter.rect_filled(zoom, 0.0, Color32::BLACK);
        // 只画在图像范围内的部分（靠近屏幕边缘时一部分是黑的）。
        let bounds = self.capture.bounds();
        let visible = source.intersect(bounds);
        if visible.is_positive() {
            let dest = Rect::from_min_max(
                zoom.min + (visible.min - source.min) * MAGNIFIER_CELL,
                zoom.min + (visible.max - source.min) * MAGNIFIER_CELL,
            );
            let uv = Rect::from_min_max(
                pos2(
                    visible.min.x / bounds.width(),
                    visible.min.y / bounds.height(),
                ),
                pos2(
                    visible.max.x / bounds.width(),
                    visible.max.y / bounds.height(),
                ),
            );
            painter.image(texture.id(), dest, uv, Color32::WHITE);
        }
        // 十字线与中间的像素框。
        let middle = Rect::from_min_size(
            zoom.min + Vec2::splat(half * MAGNIFIER_CELL),
            Vec2::splat(MAGNIFIER_CELL),
        );
        let cross = Color32::from_rgba_unmultiplied(0x30, 0x7b, 0xf6, 90);
        painter.rect_filled(
            Rect::from_x_y_ranges(zoom.x_range(), middle.y_range()),
            0.0,
            cross,
        );
        painter.rect_filled(
            Rect::from_x_y_ranges(middle.x_range(), zoom.y_range()),
            0.0,
            cross,
        );
        painter.rect_stroke(
            middle,
            0.0,
            Stroke::new(1.0, Color32::WHITE),
            StrokeKind::Inside,
        );
        painter.rect_stroke(
            zoom,
            0.0,
            Stroke::new(1.0, Color32::from_white_alpha(60)),
            StrokeKind::Inside,
        );

        // 每行高 `LINE`，文字在行内上下居中、左右居中。
        let centered = |galley: Arc<Galley>, top: f32| {
            let pos = pos2(
                frame.center().x - galley.size().x / 2.0,
                top + (LINE - galley.size().y) / 2.0,
            );
            painter.galley(pos, galley, text_color);
        };
        let mut top = zoom.bottom() + 6.0;
        centered(position, top);
        top += LINE;
        if let Some((rgba, galley)) = color {
            // 色块和颜色值作为一个整体居中。
            let left = frame.center().x - color_width / 2.0;
            let swatch =
                Rect::from_min_size(pos2(left, top + (LINE - SWATCH) / 2.0), Vec2::splat(SWATCH));
            painter.rect_filled(swatch, 2.0, Color32::from_rgb(rgba[0], rgba[1], rgba[2]));
            painter.rect_stroke(
                swatch,
                2.0,
                Stroke::new(1.0, Color32::from_white_alpha(120)),
                StrokeKind::Inside,
            );
            let pos = pos2(
                swatch.right() + SWATCH_GAP,
                top + (LINE - galley.size().y) / 2.0,
            );
            painter.galley(pos, galley, text_color);
            top += LINE;
        }
        for hint in hints {
            centered(hint, top);
            top += LINE;
        }
    }
}

/// 截图在窗口中的位置：像素坐标与点之间的换算。
struct View {
    screen: Rect,
    /// 每个点对应的像素数（Retina 屏为 2）。
    scale: f32,
}

impl View {
    fn new(screen: Rect, bounds: Rect) -> Self {
        Self {
            screen,
            scale: bounds.width() / screen.width().max(1.0),
        }
    }

    fn to_pixels(&self, point: Pos2) -> Pos2 {
        ((point - self.screen.min) * self.scale).to_pos2()
    }

    fn to_point(&self, pixel: Pos2) -> Pos2 {
        self.screen.min + pixel.to_vec2() / self.scale
    }

    fn to_points(&self, rect: Rect) -> Rect {
        Rect::from_min_max(self.to_point(rect.min), self.to_point(rect.max))
    }
}

/// 在高亮区域之外盖一层半透明遮罩。
fn paint_mask(painter: &Painter, screen: Rect, hole: Rect) {
    let hole = hole.intersect(screen);
    let parts = [
        Rect::from_min_max(screen.min, pos2(screen.max.x, hole.min.y)),
        Rect::from_min_max(pos2(screen.min.x, hole.max.y), screen.max),
        Rect::from_min_max(pos2(screen.min.x, hole.min.y), pos2(hole.min.x, hole.max.y)),
        Rect::from_min_max(pos2(hole.max.x, hole.min.y), pos2(screen.max.x, hole.max.y)),
    ];
    for part in parts.into_iter().filter(|part| part.is_positive()) {
        painter.rect_filled(part, 0.0, MASK);
    }
}

/// 选区的八个控制点。
fn paint_handles(painter: &Painter, rect: Rect) {
    for handle in Handle::RESIZE {
        let square = Rect::from_center_size(handle.anchor(rect), Vec2::splat(HANDLE_SIZE));
        painter.rect_filled(square, 1.0, Color32::WHITE);
        painter.rect_stroke(square, 1.0, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
    }
}

/// 选区左上角上方的尺寸（像素）。
fn size_badge(painter: &Painter, screen: Rect, hole: Rect, size: Vec2) {
    badge(
        painter,
        screen,
        hole,
        &format!("{} × {}", size.x as u32, size.y as u32),
    );
}

/// 选区左上角上方的一行文字（放不下时放在选区里面）。
fn badge(painter: &Painter, screen: Rect, hole: Rect, text: &str) {
    let text = text.to_owned();
    let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), Color32::WHITE);
    let badge = galley.size() + vec2(12.0, 6.0);
    let mut min = pos2(hole.left(), hole.top() - badge.y - 6.0);
    if min.y < screen.top() {
        min.y = hole.top() + 6.0;
        min.x += 6.0;
    }
    min.x = min
        .x
        .clamp(screen.left(), (screen.right() - badge.x).max(screen.left()));
    let rect = Rect::from_min_size(min, badge);
    painter.rect_filled(rect, 4.0, BADGE);
    painter.galley(rect.min + vec2(6.0, 3.0), galley, Color32::WHITE);
}

/// 输入里截屏遮罩 `viewport` 的截图事件（导出时请求的那一帧画面）。
fn screenshot_event(events: &[Event], viewport: ViewportId) -> Option<Arc<ColorImage>> {
    events.iter().find_map(|event| match event {
        Event::Screenshot {
            viewport_id, image, ..
        } if *viewport_id == viewport => Some(image.clone()),
        _ => None,
    })
}
