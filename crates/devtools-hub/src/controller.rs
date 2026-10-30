//! 显示/隐藏面板的入口，供热键、托盘图标与 UI 共同使用。
//!
//! 热键（macOS 的 Carbon 事件、Windows 的 WM_HOTKEY）和托盘菜单的回调都在主线程执行，
//! 可以直接操作窗口；万一不在主线程，就把请求转给 UI 的 `logic()` 处理。

use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use crate::platform::panel::{self, Anchor};
use crate::platform::TrayRect;
use crate::shared::{AppEvent, Shared};

/// 主线程的 id，用来判断回调是否可以直接操作窗口。
static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();
/// 面板上次因失焦自动隐藏的时间，见 [`note_auto_hide`]。
static LAST_AUTO_HIDE: Mutex<Option<Instant>> = Mutex::new(None);

/// 在 `main` 中、启动事件循环之前调用，记录主线程。
pub fn init_main_thread() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
}

/// 当前是否在主线程。
fn on_main_thread() -> bool {
    MAIN_THREAD
        .get()
        .is_some_and(|id| *id == std::thread::current().id())
}

/// 面板因失去焦点而自动隐藏时调用。
///
/// Windows 上点击托盘图标时，按下鼠标就会让面板失去焦点并隐藏，
/// 随后的点击事件不应再把它弹出来，否则托盘图标永远无法收起面板。
pub fn note_auto_hide() {
    *LAST_AUTO_HIDE.lock().unwrap() = Some(Instant::now());
}

/// 面板是否刚刚（400 毫秒内）因失焦自动隐藏。
fn just_auto_hidden() -> bool {
    LAST_AUTO_HIDE
        .lock()
        .unwrap()
        .is_some_and(|at| at.elapsed() < Duration::from_millis(400))
}

/// 快捷键：面板在前台则隐藏，否则按设置的位置显示。
///
/// 收起一律走事件（`AppEvent::HidePanel`，§4.4）：隐藏还要保存尺寸、通知搜索模块、
/// 复位窗口策略，这些都属于 UI 线程的页面状态。
pub fn toggle_panel(shared: &Shared) {
    if !on_main_thread() {
        shared.post(AppEvent::TogglePanel);
        return;
    }
    if panel::is_active() {
        shared.post(AppEvent::HidePanel);
    } else {
        show_panel(shared, false);
    }
}

/// 显示面板；`open_settings` 为 true 时直接打开设置页。
pub fn show_panel(shared: &Shared, open_settings: bool) {
    if !on_main_thread() {
        shared.post(if open_settings {
            AppEvent::OpenSettings
        } else {
            AppEvent::TogglePanel
        });
        return;
    }
    panel::show(Anchor::Placement(shared.placement()), shared.panel_size());
    shared.post(if open_settings {
        AppEvent::OpenSettings
    } else {
        AppEvent::PanelShown
    });
}

/// 点击托盘 / 菜单栏图标：面板显示中则隐藏，否则在图标旁弹出。
pub fn toggle_from_status_item(shared: &Shared, tray: Option<TrayRect>) {
    if !on_main_thread() {
        shared.post(AppEvent::TogglePanel);
        return;
    }
    if panel::is_visible() {
        shared.post(AppEvent::HidePanel);
    } else if !just_auto_hidden() {
        panel::show(Anchor::StatusItem(tray), shared.panel_size());
        shared.post(AppEvent::PanelShown);
    }
}
