//! 鼠标中键快捷面板的触发。
//!
//! 对应 uTools 的“鼠标按键弹出：按下鼠标中键”：平台层（`platform::mouse`）在监听线程上
//! 收到中键按下，这里决定是否处理；处理时把事件转给 UI 线程，由 UI 线程模拟复制、
//! 读取选中内容（见 `selection`）并在鼠标处弹出快捷面板。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::platform::{mouse, workspace};
use crate::shared::{AppEvent, Shared};

/// 启动监听。只需要调用一次；之后由 `Shared::middle_click` 控制是否拦截。
pub fn start(shared: Arc<Shared>) {
    mouse::spawn_middle_click_listener(move || {
        if !shared.middle_click.load(Ordering::Relaxed) {
            return false;
        }
        // 本应用在前台时（例如正在使用 JSON 编辑器）不拦截，中键照常交给本应用的窗口。
        if workspace::is_self_active() {
            return false;
        }
        shared.post(AppEvent::MiddleClick);
        true
    });
}
