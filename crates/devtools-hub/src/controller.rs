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

static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();
static LAST_AUTO_HIDE: Mutex<Option<Instant>> = Mutex::new(None);

/// 在 `main` 中、启动事件循环之前调用，记录主线程。
pub fn init_main_thread() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
}

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

fn just_auto_hidden() -> bool {
    LAST_AUTO_HIDE
        .lock()
        .unwrap()
        .is_some_and(|at| at.elapsed() < Duration::from_millis(400))
}

pub fn toggle_panel(shared: &Shared) {
    if !on_main_thread() {
        shared.post(AppEvent::TogglePanel);
        return;
    }
    if panel::is_active() {
        panel::hide(true);
    } else {
        show_panel(shared, false);
    }
}

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
        panel::hide(true);
    } else if !just_auto_hidden() {
        panel::show(Anchor::StatusItem(tray), shared.panel_size());
        shared.post(AppEvent::PanelShown);
    }
}
