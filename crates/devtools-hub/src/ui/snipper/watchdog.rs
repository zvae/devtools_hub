//! 截屏遮罩的看门狗：遮罩盖住整个显示器、层级在菜单栏和程序坞之上，一旦它不再绘制、不响应输入，
//! 整个屏幕就点不了也退不出（实测只能强制关机）。
//!
//! 遮罩正常时，鼠标在它上面移动一定会重绘（放大镜、十字光标跟着鼠标）。所以后台线程定时看鼠标位置：
//! 鼠标在遮罩所在的显示器上动了、之后超过 [`STALL`] 都没有重绘，就认为遮罩卡住了，
//! 交给平台层收起遮罩、把焦点还回去（`floating::rescue_overlay`，不经过 eframe），再通知主界面清理截屏状态。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use egui::ViewportId;

use crate::platform::{floating, screen};

/// 鼠标动了之后多久还没有重绘，就认为遮罩卡住了。
const STALL: Duration = Duration::from_secs(10);
/// 多久看一次鼠标位置。
const POLL: Duration = Duration::from_millis(200);
/// 鼠标移动超过这个距离（平台单位）才算动了。
const MOVE_THRESHOLD: f64 = 2.0;

/// 正在看守的遮罩。
struct Watch {
    /// 这次看守的编号：遮罩销毁时只停止自己的看守（见 [`stop`]）。
    id: u64,
    /// 遮罩铺满的显示器（全局坐标，平台单位）：鼠标在别的显示器上移动不会让遮罩重绘。
    frame: [f64; 4],
    /// 收起遮罩后用它通知主界面运行一帧。
    ctx: egui::Context,
}

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// 遮罩最近一次重绘的时间（相对 [`epoch`] 的毫秒数）。
static LAST_FRAME: AtomicU64 = AtomicU64::new(0);
/// 看门狗收起过卡住的遮罩，主界面还没处理（见 [`take_rescued`]）。
static RESCUED: AtomicBool = AtomicBool::new(false);
static THREAD: OnceLock<Thread> = OnceLock::new();

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64
}

/// 遮罩摆好、显示出来后开始看守，返回看守编号（交给 [`stop`]）。
pub fn start(ctx: egui::Context, frame: [f64; 4]) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    frame_painted();
    *WATCH.lock().unwrap() = Some(Watch { id, frame, ctx });
    THREAD
        .get_or_init(|| {
            thread::Builder::new()
                .name("snip-watchdog".into())
                .spawn(run)
                .expect("创建截屏看门狗线程失败")
                .thread()
                .clone()
        })
        .unpark();
    id
}

/// 停止看守编号为 `id` 的遮罩（遮罩已经收起或销毁）。
pub fn stop(id: u64) {
    let mut watch = WATCH.lock().unwrap();
    if watch.as_ref().is_some_and(|watch| watch.id == id) {
        *watch = None;
    }
}

/// 遮罩重绘了一帧（每次运行遮罩的界面时调用）。
pub fn frame_painted() {
    LAST_FRAME.store(now_ms(), Ordering::Relaxed);
}

/// 看门狗是否收起过卡住的遮罩；取走后复位。主界面据此丢掉截屏状态。
pub fn take_rescued() -> bool {
    RESCUED.swap(false, Ordering::Relaxed)
}

fn run() {
    let mut detector = Detector::default();
    loop {
        let Some(frame) = WATCH.lock().unwrap().as_ref().map(|watch| watch.frame) else {
            detector = Detector::default();
            thread::park();
            continue;
        };
        thread::sleep(POLL);
        let now = now_ms();
        let last_frame = LAST_FRAME.load(Ordering::Relaxed);
        let cursor = screen::cursor().map(|(point, _)| point);
        if detector.sample(now, last_frame, cursor, frame) {
            rescue(now.saturating_sub(last_frame));
        }
    }
}

/// 判断遮罩是否卡住：鼠标在遮罩所在的显示器上动了，之后超过 [`STALL`] 都没有重绘。
#[derive(Default)]
struct Detector {
    last_cursor: Option<[f64; 2]>,
    /// 最近一帧之后第一次看到鼠标移动的时间（毫秒）。
    moved_since: Option<u64>,
}

impl Detector {
    /// 记录一次采样（时间都是毫秒），返回遮罩是否卡住了。
    fn sample(
        &mut self,
        now: u64,
        last_frame: u64,
        cursor: Option<[f64; 2]>,
        frame: [f64; 4],
    ) -> bool {
        if self.moved_since.is_some_and(|at| last_frame >= at) {
            self.moved_since = None;
        }
        if let (Some(cursor), Some(last)) = (cursor, self.last_cursor) {
            let moved = (cursor[0] - last[0]).abs() + (cursor[1] - last[1]).abs() > MOVE_THRESHOLD;
            if moved && contains(frame, cursor) && self.moved_since.is_none() {
                self.moved_since = Some(now);
            }
        }
        self.last_cursor = cursor;
        let stalled = self
            .moved_since
            .is_some_and(|at| now.saturating_sub(at) > STALL.as_millis() as u64);
        if stalled {
            self.moved_since = None;
        }
        stalled
    }
}

fn contains(frame: [f64; 4], point: [f64; 2]) -> bool {
    let [x, y, width, height] = frame;
    (x..x + width).contains(&point[0]) && (y..y + height).contains(&point[1])
}

/// 遮罩卡住了：收起它、把焦点还回去，再让主界面清理截屏状态。
fn rescue(idle_ms: u64) {
    let Some(watch) = WATCH.lock().unwrap().take() else {
        return;
    };
    tracing::warn!("截屏遮罩 {idle_ms}ms 没有重绘，鼠标移动也没有反应，强制收起");
    floating::rescue_overlay();
    RESCUED.store(true, Ordering::Relaxed);
    watch.ctx.request_repaint_of(ViewportId::ROOT);
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: [f64; 4] = [0.0, 0.0, 1920.0, 1080.0];

    /// 每 200ms 采样一次，鼠标依次经过 `cursors`，遮罩最后一次重绘在 `last_frame`；返回第几次采样判定卡住。
    fn stalled_at(cursors: &[[f64; 2]], last_frame: impl Fn(u64) -> u64) -> Option<usize> {
        let mut detector = Detector::default();
        cursors.iter().enumerate().find_map(|(i, cursor)| {
            let now = 10_000 + i as u64 * 200;
            detector
                .sample(now, last_frame(now), Some(*cursor), FRAME)
                .then_some(i)
        })
    }

    #[test]
    fn mouse_moving_without_frames_is_stalled() {
        let samples = (STALL.as_millis() / 200) as usize;
        let mut cursors = vec![[100.0, 100.0], [200.0, 100.0]];
        cursors.extend(std::iter::repeat_n([200.0, 100.0], samples + 10));
        // 第 1 次采样看到移动，之后超过 STALL（再过 samples + 1 次采样）判定卡住。
        assert_eq!(stalled_at(&cursors, |_| 0), Some(samples + 2));
    }

    #[test]
    fn frames_after_moving_are_fine() {
        let cursors: Vec<_> = (0..40).map(|i| [i as f64 * 10.0, 100.0]).collect();
        // 遮罩一直在重绘（每次采样前都画过一帧）。
        assert_eq!(stalled_at(&cursors, |now| now - 50), None);
    }

    #[test]
    fn idle_mouse_or_other_display_is_fine() {
        assert_eq!(stalled_at(&[[100.0, 100.0]; 40], |_| 0), None);
        let other: Vec<_> = (0..40).map(|i| [3000.0 + i as f64 * 10.0, 100.0]).collect();
        assert_eq!(stalled_at(&other, |_| 0), None);
    }

    #[test]
    fn contains_checks_display_frame() {
        let frame = [1920.0, 0.0, 1440.0, 900.0];
        assert!(contains(frame, [2000.0, 10.0]));
        assert!(!contains(frame, [100.0, 10.0]));
        assert!(!contains(frame, [2000.0, 900.0]));
    }
}
