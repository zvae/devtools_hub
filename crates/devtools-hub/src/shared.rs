//! UI 线程、监听线程、热键与菜单栏回调之间共享的状态。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use devtools_clipboard::CaptureRules;

use crate::platform::panel::Placement;
use crate::settings::Settings;

/// 从热键、菜单栏发往 UI 的事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppEvent {
    /// 面板刚刚显示：重置搜索、刷新列表、聚焦搜索框。
    PanelShown,
    /// 需要在主线程显示/切换面板（回调不在主线程时的兜底）。
    TogglePanel,
    OpenSettings,
    PauseChanged,
}

/// 监听线程需要的配置快照。
#[derive(Clone, Debug)]
pub struct MonitorConfig {
    pub rules: CaptureRules,
    pub retention_ms: Option<i64>,
    pub max_items: Option<usize>,
}

pub struct Shared {
    pub paused: AtomicBool,
    pub poll_interval_ms: AtomicU64,
    /// 本应用自己写剪贴板时产生的 changeCount，监听线程遇到它直接跳过。
    pub ignore_change_count: AtomicIsize,
    /// 本应用正在写剪贴板（Windows 上写入过程中序号会多次变化），监听线程暂停读取。
    pub writing: AtomicBool,
    /// 每记录一条就加一，UI 据此判断是否需要刷新列表。
    pub revision: AtomicU64,
    pub stop: AtomicBool,
    config: Mutex<MonitorConfig>,
    placement: Mutex<Placement>,
    events: Mutex<VecDeque<AppEvent>>,
    ctx: OnceLock<egui::Context>,
}

impl Shared {
    pub fn new(settings: &Settings) -> Arc<Self> {
        Arc::new(Self {
            paused: AtomicBool::new(settings.paused),
            poll_interval_ms: AtomicU64::new(settings.poll_interval_ms),
            ignore_change_count: AtomicIsize::new(-1),
            writing: AtomicBool::new(false),
            revision: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            config: Mutex::new(MonitorConfig {
                rules: settings.rules.clone(),
                retention_ms: settings.retention.millis(),
                max_items: settings.max_items(),
            }),
            placement: Mutex::new(settings.placement),
            events: Mutex::new(VecDeque::new()),
            ctx: OnceLock::new(),
        })
    }

    /// 设置变化后同步给其他线程。
    pub fn apply_settings(&self, settings: &Settings) {
        self.paused.store(settings.paused, Ordering::Relaxed);
        self.poll_interval_ms
            .store(settings.poll_interval_ms, Ordering::Relaxed);
        *self.config.lock().unwrap() = MonitorConfig {
            rules: settings.rules.clone(),
            retention_ms: settings.retention.millis(),
            max_items: settings.max_items(),
        };
        *self.placement.lock().unwrap() = settings.placement;
    }

    pub fn config(&self) -> MonitorConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn placement(&self) -> Placement {
        *self.placement.lock().unwrap()
    }

    pub fn set_ctx(&self, ctx: egui::Context) {
        let _ = self.ctx.set(ctx);
    }

    pub fn request_repaint(&self) {
        if let Some(ctx) = self.ctx.get() {
            ctx.request_repaint();
        }
    }

    pub fn post(&self, event: AppEvent) {
        self.events.lock().unwrap().push_back(event);
        self.request_repaint();
    }

    pub fn drain_events(&self) -> Vec<AppEvent> {
        self.events.lock().unwrap().drain(..).collect()
    }

    /// 由本应用写剪贴板：写入期间监听线程暂停，写完登记最终的序号让它跳过。
    pub fn write_pasteboard(&self, write: impl FnOnce() -> isize) {
        self.writing.store(true, Ordering::SeqCst);
        let count = write();
        self.ignore_change_count.store(count, Ordering::SeqCst);
        self.writing.store(false, Ordering::SeqCst);
    }

    pub fn bump_revision(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
        self.request_repaint();
    }
}
