//! 贴图窗口（参照 Snipaste 的 `Paster`）：把图片放在置顶的无边框小窗口里，可以拖动、缩放、
//! 调整不透明度、旋转翻转、复制和保存。
//!
//! 每张贴图是一个 egui 的延迟视口（deferred viewport）：有自己的窗口和重绘，鼠标在一张贴图上移动时
//! 不会重绘其他窗口。主界面每帧调用 [`Pin::show`] 维持这些视口，回调里只画图片、处理鼠标和按键；
//! 需要剪贴板、保存对话框或弹出菜单的操作经 [`AppEvent::Pin`] 交给主界面执行。
//!
//! 默认交互与 Snipaste 一致：拖动移动，滚轮以鼠标为中心缩放，⌘ / Ctrl + 滚轮调整不透明度，
//! 双击或 esc 关闭（Snipaste 的提示：“双击贴图窗口会使其关闭”），右键菜单里有其余操作。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{
    vec2, Color32, Event, FontId, Id, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect,
    Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, ViewportBuilder,
    ViewportCommand, ViewportId, WindowLevel,
};
use image::RgbaImage;
use tray_icon::menu::accelerator::Code;

use super::native_menu::{self, Entry};
use crate::platform::floating;
use crate::shared::{AppEvent, PinRequest, Shared};
use crate::snip;

/// 贴图获得焦点、鼠标悬停时的边框颜色（与界面的强调色一致）。
const ACCENT: Color32 = Color32::from_rgb(0x30, 0x7b, 0xf6);
/// 缩放范围（相对原始大小）。
const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 8.0;
/// 贴图窗口的最小边长（点），缩得再小就不好拖动了。
const MIN_SIDE: f32 = 16.0;
/// 滚轮每格缩放 10%，⌘ / Ctrl + 滚轮每格改变 10% 不透明度。
const ZOOM_STEP: f32 = 1.1;
const OPACITY_STEP: f32 = 0.1;
/// 触控板等按像素滚动时，多少点算一格。
const POINTS_PER_STEP: f32 = 40.0;
/// 改变窗口大小后多久之内，以自己设置的位置为准（egui 报告的窗口位置要等窗口事件送回来才更新，
/// 连续缩放时可能还是上一次改变之前的，以它为准算出的位置会来回跳）。
const FRAME_TRUST: Duration = Duration::from_millis(400);
/// 提示文字显示的时长。
const TIP_DURATION: Duration = Duration::from_millis(1100);

/// 右键菜单和快捷键对应的操作。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PinAction {
    Copy,
    Save,
    /// 旋转 90 度，true 为顺时针。
    Rotate(bool),
    /// 翻转，true 为水平翻转。
    Flip(bool),
    /// 缩放到指定百分比。
    Zoom(u16),
    /// 不透明度（百分比）。
    Opacity(u8),
    ToggleTopmost,
    ToggleShadow,
    /// 编辑标注（由主界面打开截屏遮罩）。
    Edit,
    Close,
    CloseAll,
}

/// 一张贴图。
pub struct Pin {
    pub id: u64,
    viewport: ViewportId,
    builder: ViewportBuilder,
    state: Arc<Mutex<PinState>>,
}

/// 贴图窗口的状态，由主界面和窗口回调共享（都在主线程，锁只是为了满足延迟视口回调的 `Send + Sync`）。
struct PinState {
    id: u64,
    title: String,
    viewport: ViewportId,
    image: Arc<RgbaImage>,
    /// 图片有透明像素时在下面垫白底。
    has_alpha: bool,
    texture: Option<TextureHandle>,
    texture_dirty: bool,
    /// 缩放为 100% 时的窗口大小（点，随旋转交换宽高）。
    base: Vec2,
    zoom: f32,
    opacity: f32,
    topmost: bool,
    shadow: bool,
    // 已经应用到窗口上的状态：与上面的值不同时由回调发出视口命令或调用平台接口。
    applied_size: Vec2,
    applied_opacity: f32,
    applied_topmost: bool,
    applied_shadow: bool,
    /// 平台层是否已经设置好窗口（换成不激活应用的面板等）。
    configured: bool,
    /// 窗口在桌面上的位置（逻辑坐标，上一帧），编辑标注时用。
    screen_rect: Option<Rect>,
    /// 下一次改变大小时保持不动的点（相对窗口左上角）；为空时以中心缩放。
    anchor: Option<Vec2>,
    /// 最近一次缩放设置的窗口位置与大小（逻辑坐标）和设置的时间（见 [`FRAME_TRUST`]）。
    set_frame: Option<(Rect, Instant)>,
    /// 窗口边长的上限（点）：物理像素不能超过显卡支持的最大纹理边长，否则 wgpu 配置 surface 时校验失败、
    /// 程序直接崩溃（长截图贴图放大时出现过）。每帧按 `max_texture_side` 和缩放比例更新。
    max_side: f32,
    tip: Option<(String, Instant)>,
    closed: bool,
}

impl Pin {
    /// 创建贴图：`base` 是 100% 时的大小（点），`zoom` 是初始缩放，窗口左上角在 `top_left`（逻辑坐标）。
    pub fn new(id: u64, image: RgbaImage, base: Vec2, zoom: f32, top_left: Pos2) -> Self {
        let title = format!("DevTools Hub 贴图 {id}");
        let viewport = ViewportId::from_hash_of(("devtools-hub-pin", id));
        let size = (base * zoom).round().max(Vec2::splat(MIN_SIDE));
        let builder = ViewportBuilder::default()
            .with_title(title.clone())
            .with_decorations(false)
            .with_position(top_left)
            .with_inner_size(size)
            .with_resizable(false)
            .with_has_shadow(true)
            .with_taskbar(false)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        let state = PinState {
            id,
            title,
            viewport,
            has_alpha: snip::has_alpha(&image),
            image: Arc::new(image),
            texture: None,
            texture_dirty: false,
            base,
            zoom,
            opacity: 1.0,
            topmost: true,
            shadow: true,
            applied_size: size,
            applied_opacity: 1.0,
            applied_topmost: true,
            applied_shadow: true,
            configured: false,
            screen_rect: None,
            anchor: None,
            set_frame: None,
            max_side: f32::INFINITY,
            tip: None,
            closed: false,
        };
        Self {
            id,
            viewport,
            builder,
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// 维持贴图窗口（主界面每帧调用，不调用的那一帧 egui 会关闭窗口）。
    pub fn show(&self, ctx: &egui::Context, shared: &Arc<Shared>) {
        if !self.state.lock().unwrap().configured {
            // 窗口在下一次运行界面时由 eframe 创建，之后还要画一帧才会显示：在那之前主界面和贴图都要继续重绘
            // （面板隐藏时没有别的事件会触发它们）。
            ctx.request_repaint();
            ctx.request_repaint_of(self.viewport);
        }
        let state = self.state.clone();
        let shared = shared.clone();
        ctx.show_viewport_deferred(self.viewport, self.builder.clone(), move |ui, _| {
            state.lock().unwrap().ui(ui, &shared);
        });
    }

    /// 是否已经关闭（主界面随后不再维持它的窗口）。
    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }

    /// 窗口标题（平台层按标题找到窗口，例如弹出右键菜单）。
    pub fn title(&self) -> String {
        self.state.lock().unwrap().title.clone()
    }

    /// 当前（旋转、翻转之后）的图片。
    pub fn image(&self) -> Arc<RgbaImage> {
        self.state.lock().unwrap().image.clone()
    }

    /// 执行操作（复制、保存、关闭全部由主界面处理，这里忽略），并重绘窗口。
    pub fn apply(&self, ctx: &egui::Context, action: PinAction) {
        self.state.lock().unwrap().apply(ctx, action);
    }

    /// 窗口在桌面上的位置（逻辑坐标）；还没显示过时为空。
    pub fn screen_rect(&self) -> Option<Rect> {
        self.state.lock().unwrap().screen_rect
    }

    /// 换成编辑过的图片：`size` 是它在屏幕上的大小（点），按 100% 显示，窗口大小不变。
    pub fn replace_image(&self, ctx: &egui::Context, image: RgbaImage, size: Vec2) {
        let mut state = self.state.lock().unwrap();
        state.has_alpha = snip::has_alpha(&image);
        state.image = Arc::new(image);
        state.texture_dirty = true;
        state.base = size;
        state.zoom = 1.0;
        state.anchor = None;
        ctx.request_repaint_of(self.viewport);
    }

    /// 在贴图上显示一条短暂的提示（例如“已复制”）。
    pub fn tip(&self, ctx: &egui::Context, text: &str) {
        self.state.lock().unwrap().show_tip(text);
        ctx.request_repaint_of(self.viewport);
    }

    /// 右键菜单。
    pub fn menu_entries(&self) -> Vec<Entry<PinAction>> {
        let state = self.state.lock().unwrap();
        let zoom = (state.zoom * 100.0).round() as u16;
        let opacity = (state.opacity * 100.0).round() as u8;
        let mut entries = vec![
            Entry::shortcut("编辑标注", native_menu::NONE, Code::Space, PinAction::Edit),
            Entry::Separator,
            Entry::shortcut("复制", native_menu::COMMAND, Code::KeyC, PinAction::Copy),
            Entry::shortcut("保存…", native_menu::COMMAND, Code::KeyS, PinAction::Save),
            Entry::Separator,
            Entry::Submenu {
                label: "旋转与翻转".into(),
                entries: vec![
                    Entry::shortcut(
                        "顺时针旋转 90°",
                        native_menu::NONE,
                        Code::KeyR,
                        PinAction::Rotate(true),
                    ),
                    Entry::shortcut(
                        "逆时针旋转 90°",
                        native_menu::SHIFT,
                        Code::KeyR,
                        PinAction::Rotate(false),
                    ),
                    Entry::Separator,
                    Entry::item("水平翻转", PinAction::Flip(true)),
                    Entry::item("垂直翻转", PinAction::Flip(false)),
                ],
            },
            Entry::Submenu {
                label: format!("缩放（{zoom}%）"),
                entries: [25, 50, 75, 100, 150, 200, 300]
                    .into_iter()
                    .map(|percent| Entry::Check {
                        label: format!("{percent}%"),
                        checked: percent == zoom,
                        action: PinAction::Zoom(percent),
                    })
                    .collect(),
            },
        ];
        if floating::OPACITY_SUPPORTED {
            entries.push(Entry::Submenu {
                label: format!("不透明度（{opacity}%）"),
                entries: [100, 90, 80, 70, 50, 30]
                    .into_iter()
                    .map(|percent| Entry::Check {
                        label: format!("{percent}%"),
                        checked: percent == opacity,
                        action: PinAction::Opacity(percent),
                    })
                    .collect(),
            });
        }
        entries.push(Entry::Separator);
        entries.push(Entry::Check {
            label: "置顶".into(),
            checked: state.topmost,
            action: PinAction::ToggleTopmost,
        });
        if cfg!(target_os = "macos") {
            entries.push(Entry::Check {
                label: "阴影".into(),
                checked: state.shadow,
                action: PinAction::ToggleShadow,
            });
        }
        entries.extend([
            Entry::Separator,
            Entry::shortcut("关闭", native_menu::NONE, Code::Escape, PinAction::Close),
            Entry::item("关闭所有贴图", PinAction::CloseAll),
        ]);
        entries
    }
}

impl PinState {
    /// 窗口回调：同步窗口状态、绘制图片、处理鼠标与按键。
    fn ui(&mut self, ui: &mut Ui, shared: &Arc<Shared>) {
        let ctx = ui.ctx().clone();
        if self.closed {
            // 等主界面关闭窗口（下一帧）期间照常画出图片：不画的话窗口会变成一块灰色。
            let texture = self.texture(&ctx);
            ui.painter().image(
                texture.id(),
                ctx.viewport_rect(),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            return;
        }
        if !self.configured {
            self.configured = floating::configure_pin(&self.title);
            ctx.request_repaint();
        }
        self.max_side = ctx.input(|i| i.max_texture_side) as f32 / ctx.pixels_per_point();
        self.zoom = self.zoom.min(self.zoom_range().1);
        let texture = self.texture(&ctx);
        let rect = ctx.viewport_rect();
        let painter = ui.painter().with_clip_rect(rect);
        if self.has_alpha {
            painter.rect_filled(rect, 0.0, Color32::WHITE);
        }
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );

        self.screen_rect = ctx.input(|i| i.viewport().outer_rect);
        let response = ui.interact(rect, Id::new(("pin", self.id)), Sense::click_and_drag());
        if response.drag_started_by(PointerButton::Primary) {
            // 拖动会移动窗口，之后以 egui 报告的位置为准。
            self.set_frame = None;
            if !floating::start_drag(&self.title) {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
        }
        if response.double_clicked() {
            self.close(&ctx);
            return;
        }
        if response.secondary_clicked() {
            shared.post(AppEvent::Pin(self.id, PinRequest::Menu));
        }
        self.wheel(ui, &ctx, rect);
        self.keys(ui, &ctx, shared);
        // 本帧的缩放、旋转等变化立即应用到窗口上。
        self.sync_window(&ctx);

        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        if response.hovered() || focused {
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
        }
        self.paint_tip(&painter, &ctx, rect);
    }

    /// 图片的纹理（第一次绘制、旋转翻转之后上传）。超过显卡支持的最大边长时（例如长截图）缩小后上传。
    fn texture(&mut self, ctx: &egui::Context) -> TextureHandle {
        let max = ctx.input(|i| i.max_texture_side);
        match &mut self.texture {
            Some(texture) if !self.texture_dirty => texture.clone(),
            Some(texture) => {
                texture.set(
                    snip::texture_image(&self.image, max),
                    TextureOptions::LINEAR,
                );
                self.texture_dirty = false;
                texture.clone()
            }
            None => {
                let texture = ctx.load_texture(
                    format!("pin-{}", self.id),
                    snip::texture_image(&self.image, max),
                    TextureOptions::LINEAR,
                );
                self.texture = Some(texture.clone());
                self.texture_dirty = false;
                texture
            }
        }
    }

    /// 把缩放、不透明度、置顶、阴影的变化应用到窗口上。
    fn sync_window(&mut self, ctx: &egui::Context) {
        let size = (self.base * self.zoom).round().max(Vec2::splat(MIN_SIDE));
        if (size - self.applied_size).abs().max_elem() >= 1.0 {
            let outer = self
                .set_frame
                .filter(|(_, at)| at.elapsed() < FRAME_TRUST)
                .map(|(frame, _)| frame)
                .or_else(|| ctx.input(|i| i.viewport().outer_rect));
            if let Some(outer) = outer {
                let anchor = self.anchor.take().unwrap_or(outer.size() / 2.0);
                let position = snip::zoom_about(outer, anchor, size);
                let frame = [position.x, position.y, size.x, size.y].map(f64::from);
                // 位置和大小一次设置好；没找到原生窗口（例如测试里）时退回 egui 的视口命令。
                if !floating::set_window_frame(&self.title, frame) {
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
                    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(position));
                }
                self.set_frame = Some((Rect::from_min_size(position, size), Instant::now()));
            } else {
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
            }
            self.applied_size = size;
        }
        if (self.opacity - self.applied_opacity).abs() > 0.001 {
            floating::set_opacity(&self.title, self.opacity);
            self.applied_opacity = self.opacity;
        }
        if self.topmost != self.applied_topmost {
            let level = if self.topmost {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            };
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(level));
            self.applied_topmost = self.topmost;
        }
        if self.shadow != self.applied_shadow {
            floating::set_shadow(&self.title, self.shadow);
            self.applied_shadow = self.shadow;
        }
    }

    /// 滚轮缩放（以鼠标位置为中心）、⌘ / Ctrl + 滚轮调整不透明度、触控板双指缩放。
    fn wheel(&mut self, ui: &Ui, ctx: &egui::Context, rect: Rect) {
        let (steps, opacity_steps, pinch, hover) = ui.input(|i| {
            let mut steps = 0.0;
            let mut opacity_steps = 0.0;
            let mut pinch = 1.0;
            for event in &i.events {
                match event {
                    Event::MouseWheel {
                        unit,
                        delta,
                        modifiers,
                        ..
                    } => {
                        let amount = match unit {
                            MouseWheelUnit::Point => delta.y / POINTS_PER_STEP,
                            MouseWheelUnit::Line => delta.y,
                            MouseWheelUnit::Page => delta.y * 3.0,
                        };
                        if modifiers.command {
                            opacity_steps += amount;
                        } else {
                            steps += amount;
                        }
                    }
                    Event::Zoom(factor) => pinch *= factor,
                    _ => {}
                }
            }
            (steps, opacity_steps, pinch, i.pointer.hover_pos())
        });
        let factor = ZOOM_STEP.powf(steps) * pinch;
        if (factor - 1.0).abs() > 1e-4 {
            self.anchor = hover.map(|pos| pos - rect.min);
            self.set_zoom(self.zoom * factor);
            ctx.request_repaint();
        }
        if opacity_steps != 0.0 && floating::OPACITY_SUPPORTED {
            self.opacity = (self.opacity + opacity_steps * OPACITY_STEP).clamp(0.1, 1.0);
            self.show_tip(&format!("不透明度 {}%", (self.opacity * 100.0).round()));
            ctx.request_repaint();
        }
    }

    /// 按键：esc / ⌘W 关闭，⌘C 复制，⌘S 保存，空格编辑标注，R / ⇧R 旋转，0 恢复原始大小，= / - 缩放。
    fn keys(&mut self, ui: &Ui, ctx: &egui::Context, shared: &Arc<Shared>) {
        let pressed = ui.input_mut(|i| {
            let copy = i.events.iter().any(|event| matches!(event, Event::Copy))
                || i.consume_key(Modifiers::COMMAND, Key::C);
            let shortcuts = [
                (
                    i.consume_key(Modifiers::NONE, Key::Escape),
                    PinAction::Close,
                ),
                (i.consume_key(Modifiers::COMMAND, Key::W), PinAction::Close),
                (copy, PinAction::Copy),
                (i.consume_key(Modifiers::COMMAND, Key::S), PinAction::Save),
                // 与 Snipaste 一样，空格显示标注工具条。
                (i.consume_key(Modifiers::NONE, Key::Space), PinAction::Edit),
                (
                    i.consume_key(Modifiers::SHIFT, Key::R),
                    PinAction::Rotate(false),
                ),
                (
                    i.consume_key(Modifiers::NONE, Key::R),
                    PinAction::Rotate(true),
                ),
                (
                    i.consume_key(Modifiers::NONE, Key::Num0),
                    PinAction::Zoom(100),
                ),
            ];
            let zoom_in = i.consume_key(Modifiers::NONE, Key::Equals)
                || i.consume_key(Modifiers::NONE, Key::Plus);
            let zoom_out = i.consume_key(Modifiers::NONE, Key::Minus);
            (
                shortcuts
                    .into_iter()
                    .find_map(|(hit, action)| hit.then_some(action)),
                zoom_in,
                zoom_out,
            )
        });
        let (action, zoom_in, zoom_out) = pressed;
        if zoom_in || zoom_out {
            let factor = if zoom_in { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
            self.set_zoom(self.zoom * factor);
        }
        match action {
            Some(PinAction::Copy) => shared.post(AppEvent::Pin(self.id, PinRequest::Copy)),
            Some(PinAction::Save) => shared.post(AppEvent::Pin(self.id, PinRequest::Save)),
            Some(PinAction::Edit) => shared.post(AppEvent::Pin(self.id, PinRequest::Edit)),
            Some(action) => self.apply(ctx, action),
            None => {}
        }
    }

    /// 执行不需要主界面参与的操作。
    fn apply(&mut self, ctx: &egui::Context, action: PinAction) {
        match action {
            PinAction::Rotate(clockwise) => {
                self.image = Arc::new(snip::rotate(&self.image, clockwise));
                self.base = vec2(self.base.y, self.base.x);
                self.texture_dirty = true;
            }
            PinAction::Flip(horizontal) => {
                self.image = Arc::new(snip::flip(&self.image, horizontal));
                self.texture_dirty = true;
            }
            PinAction::Zoom(percent) => {
                self.anchor = None;
                self.set_zoom(f32::from(percent) / 100.0);
            }
            PinAction::Opacity(percent) => {
                self.opacity = f32::from(percent) / 100.0;
                self.show_tip(&format!("不透明度 {percent}%"));
            }
            PinAction::ToggleTopmost => {
                self.topmost = !self.topmost;
                self.show_tip(if self.topmost {
                    "已置顶"
                } else {
                    "已取消置顶"
                });
            }
            PinAction::ToggleShadow => self.shadow = !self.shadow,
            PinAction::Close => self.close(ctx),
            PinAction::Copy | PinAction::Save | PinAction::Edit | PinAction::CloseAll => {}
        }
        ctx.request_repaint_of(self.viewport);
    }

    /// 缩放比例的范围：窗口不小于最小边长，也不超过 [`Self::max_side`]（长图放大时由后者限制）。
    fn zoom_range(&self) -> (f32, f32) {
        let max = MAX_ZOOM.min((self.max_side - 1.0) / self.base.max_elem().max(1.0));
        let min = MIN_ZOOM.max(MIN_SIDE / self.base.min_elem().max(1.0));
        (min.min(max), max)
    }

    /// 改变缩放比例（限制在 [`Self::zoom_range`] 内），并显示百分比。
    fn set_zoom(&mut self, zoom: f32) {
        let (min, max) = self.zoom_range();
        self.zoom = zoom.clamp(min, max);
        self.show_tip(&format!("{}%", (self.zoom * 100.0).round()));
    }

    /// 关闭：主界面下一帧不再维持这个视口，egui 随后销毁窗口。
    fn close(&mut self, ctx: &egui::Context) {
        self.closed = true;
        ctx.request_repaint_of(ViewportId::ROOT);
    }

    fn show_tip(&mut self, text: &str) {
        self.tip = Some((text.to_string(), Instant::now() + TIP_DURATION));
    }

    /// 窗口底部居中的提示，到期后消失。
    fn paint_tip(&mut self, painter: &egui::Painter, ctx: &egui::Context, rect: Rect) {
        let Some((text, until)) = &self.tip else {
            return;
        };
        let now = Instant::now();
        if now >= *until {
            self.tip = None;
            return;
        }
        ctx.request_repaint_after(*until - now);
        let galley =
            painter.layout_no_wrap(text.clone(), FontId::proportional(12.0), Color32::WHITE);
        let size = galley.size() + vec2(16.0, 8.0);
        let badge = Rect::from_center_size(
            Pos2::new(
                rect.center().x,
                (rect.bottom() - 8.0 - size.y / 2.0).max(rect.center().y),
            ),
            size,
        );
        painter.rect_filled(badge, size.y / 2.0, Color32::from_black_alpha(190));
        painter.galley(badge.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    //! 不创建真实窗口：直接在 egui_kittest 里绘制贴图，用模拟的鼠标、键盘事件驱动。

    use std::sync::MutexGuard;

    use super::*;
    use crate::settings::Settings;
    use egui::{pos2, TouchPhase};
    use egui_kittest::Harness;
    use image::Rgba;

    struct Test {
        pin: Pin,
        shared: Arc<Shared>,
    }

    /// 100×50 的贴图（缩放 1），左上角是一个红色像素。
    fn harness() -> Harness<'static, Test> {
        let mut image = RgbaImage::from_pixel(100, 50, Rgba([0, 128, 255, 255]));
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        let pin = Pin::new(7, image, vec2(100.0, 50.0), 1.0, Pos2::ZERO);
        let shared = Shared::new(&Settings::default());
        Harness::builder()
            .with_size(vec2(100.0, 50.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, test: &mut Test| test.pin.state.lock().unwrap().ui(ui, &test.shared),
                Test { pin, shared },
            )
    }

    /// 比显卡支持的最大纹理边长还长的贴图（长截图）：缩小后上传，不 panic；旋转后重新上传也一样。
    #[test]
    fn long_image_fits_max_texture_side() {
        let image = RgbaImage::from_pixel(50, 5000, Rgba([0, 128, 255, 255]));
        let pin = Pin::new(8, image, vec2(50.0, 5000.0), 0.05, Pos2::ZERO);
        let shared = Shared::new(&Settings::default());
        let mut harness = Harness::builder()
            .with_size(vec2(16.0, 250.0))
            .build_ui_state(
                |ui, test: &mut Test| test.pin.state.lock().unwrap().ui(ui, &test.shared),
                Test { pin, shared },
            );
        harness.step();
        let max = harness.ctx.input(|i| i.max_texture_side);
        assert!(
            max < 5000,
            "测试环境的纹理上限应小于图片高度（现在是 {max}）"
        );
        let size = state(&harness).texture.as_ref().unwrap().size();
        assert!(size[1] <= max && size[0] >= 1, "纹理 {size:?}");
        // 滚轮放大、按 0 恢复 100%：窗口都不超过纹理上限（超过时 wgpu 配置 surface 失败、程序崩溃）。
        let limit = max as f32;
        wheel(&mut harness, 100.0, Modifiers::NONE);
        assert!(state(&harness).applied_size.max_elem() < limit);
        harness.key_press(Key::Num0);
        harness.step();
        assert!(state(&harness).applied_size.max_elem() < limit);
        assert!(state(&harness).zoom < 1.0);
        harness.key_press(Key::R);
        harness.step();
        let size = state(&harness).texture.as_ref().unwrap().size();
        assert!(size[0] <= max, "旋转后的纹理 {size:?}");
        assert!(state(&harness).applied_size.max_elem() < limit);
        // 原图不变，复制、保存用的仍是原始分辨率。
        assert_eq!(state(&harness).image.dimensions(), (5000, 50));
    }

    fn state<'a>(harness: &'a Harness<'_, Test>) -> MutexGuard<'a, PinState> {
        harness.state().pin.state.lock().unwrap()
    }

    fn wheel(harness: &mut Harness<'_, Test>, lines: f32, modifiers: Modifiers) {
        harness.event(Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, lines),
            phase: TouchPhase::Move,
            modifiers,
        });
        harness.step();
    }

    fn click(harness: &mut Harness<'_, Test>, button: PointerButton, pos: Pos2) {
        harness.hover_at(pos);
        harness.step();
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            });
            harness.step();
        }
    }

    #[test]
    fn wheel_zooms_and_command_wheel_changes_opacity() {
        let mut harness = harness();
        harness.hover_at(pos2(25.0, 25.0));
        harness.step();
        wheel(&mut harness, 2.0, Modifiers::NONE);
        {
            let state = state(&harness);
            assert!((state.zoom - 1.21).abs() < 1e-4, "zoom = {}", state.zoom);
            assert_eq!(state.applied_size, vec2(121.0, 61.0));
            assert_eq!(
                state.tip.as_ref().map(|(text, _)| text.as_str()),
                Some("121%")
            );
        }
        // 缩到最小：窗口不小于最小边长。
        wheel(&mut harness, -100.0, Modifiers::NONE);
        assert_eq!(state(&harness).applied_size, vec2(32.0, 16.0));
        // 0 恢复原始大小。
        harness.key_press(Key::Num0);
        harness.step();
        assert_eq!(state(&harness).applied_size, vec2(100.0, 50.0));

        wheel(&mut harness, -2.0, Modifiers::COMMAND);
        let expected = if floating::OPACITY_SUPPORTED {
            0.8
        } else {
            1.0
        };
        assert!((state(&harness).opacity - expected).abs() < 1e-4);
    }

    #[test]
    fn keys_rotate_copy_and_close() {
        let mut harness = harness();
        harness.key_press(Key::R);
        harness.step();
        {
            let state = state(&harness);
            assert_eq!(state.image.dimensions(), (50, 100));
            assert_eq!(state.base, vec2(50.0, 100.0));
            // 顺时针旋转后，原来的左上角到了右上角。
            assert_eq!(state.image.get_pixel(49, 0).0, [255, 0, 0, 255]);
        }
        harness.key_press_modifiers(Modifiers::SHIFT, Key::R);
        harness.step();
        assert_eq!(state(&harness).image.dimensions(), (100, 50));

        harness.key_press_modifiers(Modifiers::COMMAND, Key::C);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
        harness.step();
        assert_eq!(
            harness.state().shared.drain_events(),
            vec![
                AppEvent::Pin(7, PinRequest::Copy),
                AppEvent::Pin(7, PinRequest::Save)
            ]
        );
        harness.key_press(Key::Space);
        harness.step();
        assert_eq!(
            harness.state().shared.drain_events(),
            vec![AppEvent::Pin(7, PinRequest::Edit)]
        );
        harness.key_press(Key::Escape);
        harness.step();
        assert!(harness.state().pin.is_closed());
    }

    #[test]
    fn right_click_asks_for_menu_and_double_click_closes() {
        let mut harness = harness();
        click(&mut harness, PointerButton::Secondary, pos2(50.0, 25.0));
        assert_eq!(
            harness.state().shared.drain_events(),
            vec![AppEvent::Pin(7, PinRequest::Menu)]
        );
        assert!(!harness.state().pin.is_closed());
        click(&mut harness, PointerButton::Primary, pos2(50.0, 25.0));
        click(&mut harness, PointerButton::Primary, pos2(50.0, 25.0));
        assert!(harness.state().pin.is_closed());
    }

    #[test]
    fn menu_actions_update_state() {
        let harness = harness();
        let ctx = harness.ctx.clone();
        let pin = &harness.state().pin;
        // 编辑后换图：按屏幕上的大小 100% 显示。
        pin.replace_image(&ctx, RgbaImage::new(60, 30), vec2(60.0, 30.0));
        {
            let state = pin.state.lock().unwrap();
            assert_eq!((state.base, state.zoom), (vec2(60.0, 30.0), 1.0));
            assert!(state.texture_dirty);
        }
        pin.replace_image(
            &ctx,
            {
                let mut image = RgbaImage::from_pixel(100, 50, Rgba([0, 128, 255, 255]));
                image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
                image
            },
            vec2(100.0, 50.0),
        );
        pin.apply(&ctx, PinAction::Zoom(200));
        pin.apply(&ctx, PinAction::Flip(true));
        pin.apply(&ctx, PinAction::ToggleTopmost);
        {
            let state = pin.state.lock().unwrap();
            assert_eq!(state.zoom, 2.0);
            assert_eq!(state.image.get_pixel(99, 0).0, [255, 0, 0, 255]);
            assert!(!state.topmost);
        }
        let labels: Vec<_> = pin
            .menu_entries()
            .iter()
            .filter_map(|entry| entry.label().map(str::to_string))
            .collect();
        assert_eq!(labels.first().map(String::as_str), Some("编辑标注"));
        assert!(labels.contains(&"复制".to_string()));
        assert!(labels.contains(&"缩放（200%）".to_string()));
        assert!(labels.contains(&"关闭所有贴图".to_string()));
    }
}
