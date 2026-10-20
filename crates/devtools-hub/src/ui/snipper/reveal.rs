//! 隐藏着创建的遮罩什么时候可以露出来（Windows，见 `floating::OVERLAY_STARTS_HIDDEN`）。
//!
//! 遮罩创建时的尺寸不对，由平台层用 `SetWindowPos` 铺满显示器。这一步在界面回调里同步完成，
//! winit 把随之而来的 `Resized` 排进队列，等回调返回后才交给 eframe 去调整 wgpu 交换链的尺寸；
//! 而 egui 每帧直接读窗口当前的尺寸来排版。于是摆好窗口的那一帧按铺满屏幕排版、却画进旧尺寸的交换链：
//! 只画出了截图的左上一块，DWM 再把它拉伸到整个窗口（录屏里截屏开始时整屏放大、闪一下）。
//!
//! 下一帧交换链的尺寸就对了，但“画了”不等于“显示了”：第一次截屏要上传整屏的截图，GPU 要画好一阵，
//! 这期间屏幕上还是拉伸的那一帧。所以在遮罩里放一个什么都不画的绘制回调，记下每帧画进去的交换链尺寸，
//! 等按显示器尺寸画的一帧在 GPU 上画完了才露出来。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use eframe::egui_wgpu::{self, wgpu, CallbackResources, CallbackTrait, ScreenDescriptor};
use egui::{vec2, Painter, Rect};

/// 一直等不到（比如没有用 wgpu 渲染）时，摆好窗口后最多再等这么多帧就露出来，
/// 免得遮罩一直遮着、整个屏幕却点不了。
const MAX_FRAMES: u32 = 60;

/// 等遮罩按显示器的尺寸画好一帧再露出来。
#[derive(Default)]
pub(super) struct Reveal {
    shared: Arc<Shared>,
    /// 摆好窗口后运行过的帧数。
    frames: u32,
}

#[derive(Default)]
struct Shared {
    /// 上一帧画进去的交换链尺寸（物理像素，宽在高 32 位）；还没画过时为 0。
    painted: AtomicU64,
    /// 按显示器尺寸画的一帧已经在 GPU 上画完。
    done: AtomicBool,
}

impl Reveal {
    /// 每帧调用（窗口摆好之后、露出来之前）：返回是否已经按 `size`（物理像素）画好一帧、可以露出来。
    pub(super) fn ready(&mut self, painter: &Painter, size: [f64; 2]) -> bool {
        if self.shared.done.load(Ordering::Relaxed) {
            return true;
        }
        if self.frames >= MAX_FRAMES {
            tracing::warn!("截屏遮罩：{MAX_FRAMES} 帧内没有按显示器尺寸画好一帧，直接显示");
            return true;
        }
        self.frames += 1;
        let rect = Rect::from_min_size(painter.clip_rect().min, vec2(1.0, 1.0));
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            Probe {
                shared: self.shared.clone(),
                expected: pack(size[0].round() as u32, size[1].round() as u32),
            },
        ));
        false
    }
}

fn pack(width: u32, height: u32) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

/// 什么都不画：记下这一帧的交换链尺寸；上一帧尺寸正确时，等到已经提交的工作（包括上一帧）在 GPU 上完成。
struct Probe {
    shared: Arc<Shared>,
    expected: u64,
}

impl CallbackTrait for Probe {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        _resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let [width, height] = screen_descriptor.size_in_pixels;
        let previous = self.shared.painted.swap(pack(width, height), Ordering::Relaxed);
        if previous == self.expected {
            let shared = self.shared.clone();
            queue.on_submitted_work_done(move || shared.done.store(true, Ordering::Relaxed));
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _render_pass: &mut wgpu::RenderPass<'static>,
        _resources: &CallbackResources,
    ) {
    }
}
