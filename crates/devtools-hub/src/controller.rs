//! 显示/隐藏面板的入口，供热键、菜单栏与 UI 共同使用。
//!
//! 热键（Carbon 事件）和菜单栏（NSMenu action）的回调都在主线程执行，
//! 可以直接操作窗口；万一不在主线程，就把请求转给 UI 的 `logic()` 处理。

use objc2::MainThreadMarker;

use crate::platform::panel::{self, Anchor};
use crate::shared::{AppEvent, Shared};

pub fn toggle_panel(shared: &Shared) {
    if MainThreadMarker::new().is_none() {
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
    if MainThreadMarker::new().is_none() {
        shared.post(if open_settings {
            AppEvent::OpenSettings
        } else {
            AppEvent::TogglePanel
        });
        return;
    }
    panel::show(Anchor::Placement(shared.placement()));
    shared.post(if open_settings {
        AppEvent::OpenSettings
    } else {
        AppEvent::PanelShown
    });
}

/// 点击菜单栏图标：面板显示中则隐藏，否则在图标正下方弹出。
pub fn toggle_from_status_item(shared: &Shared) {
    if MainThreadMarker::new().is_none() {
        shared.post(AppEvent::TogglePanel);
        return;
    }
    if panel::is_visible() {
        panel::hide(true);
    } else {
        panel::show(Anchor::StatusItem);
        shared.post(AppEvent::PanelShown);
    }
}
