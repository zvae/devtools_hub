//! UI 线程、监听线程、热键与菜单栏回调之间共享的状态。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use devtools_clipboard::CaptureRules;

use crate::platform::panel::Placement;
use crate::settings::Settings;
use crate::update::UpdateState;

/// 从热键、菜单栏发往 UI 的事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppEvent {
    /// 面板刚刚显示：重置搜索、刷新列表、聚焦搜索框。
    PanelShown,
    /// 需要在主线程显示/切换面板（回调不在主线程时的兜底）。
    TogglePanel,
    OpenSettings,
    PauseChanged,
    /// 在其他应用中按下了鼠标中键：读取选中内容并弹出快捷面板。
    MiddleClick,
    /// 检查更新的状态变了：刷新托盘菜单与面板上的新版本提示。
    UpdateChanged,
    /// 点击了托盘菜单的“发现新版本”：设置页滚动到底部的更新信息。
    RevealUpdate,
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
    /// 鼠标中键快捷面板是否开启（监听线程据此决定是否拦截中键）。
    pub middle_click: AtomicBool,
    /// 是否定时检查更新（检查更新线程据此决定是否检查）。
    pub auto_check_updates: AtomicBool,
    config: Mutex<MonitorConfig>,
    placement: Mutex<Placement>,
    /// 剪贴板面板的大小（逻辑像素）。窗口也用作快捷面板和 JSON 编辑器，
    /// 快捷键和托盘弹出面板时据此恢复大小。
    panel_size: Mutex<[f32; 2]>,
    update: Mutex<UpdateState>,
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
            middle_click: AtomicBool::new(settings.middle_click),
            auto_check_updates: AtomicBool::new(settings.auto_check_updates),
            config: Mutex::new(MonitorConfig {
                rules: settings.rules.clone(),
                retention_ms: settings.retention.millis(),
                max_items: settings.max_items(),
            }),
            placement: Mutex::new(settings.placement),
            panel_size: Mutex::new(settings.window_size),
            update: Mutex::new(UpdateState::default()),
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
        *self.panel_size.lock().unwrap() = settings.window_size;
        self.middle_click
            .store(settings.middle_click, Ordering::Relaxed);
        self.auto_check_updates
            .store(settings.auto_check_updates, Ordering::Relaxed);
    }

    pub fn config(&self) -> MonitorConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn placement(&self) -> Placement {
        *self.placement.lock().unwrap()
    }

    pub fn panel_size(&self) -> [f32; 2] {
        *self.panel_size.lock().unwrap()
    }

    pub fn update_state(&self) -> UpdateState {
        self.update.lock().unwrap().clone()
    }

    /// 修改检查更新的状态，并通知 UI 刷新提示。
    pub fn modify_update<R>(&self, modify: impl FnOnce(&mut UpdateState) -> R) -> R {
        let result = modify(&mut self.update.lock().unwrap());
        self.post(AppEvent::UpdateChanged);
        result
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
