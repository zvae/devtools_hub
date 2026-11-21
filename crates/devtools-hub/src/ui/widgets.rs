//! 多个页面共用的小控件。
//!
//! 平台无关的通用控件（图标按钮、带文字的按钮、固定高度的行、键帽）来自共用模块
//! `devtools-ui`，此处再导出以保持 `ui::widgets` 的调用路径不变；只有依赖窗口平台层的
//! 拖动区域留在本模块。

use egui::{Rect, Sense, Ui};

use crate::platform::panel as native_panel;

pub use devtools_ui::widgets::*;

/// 栏的空白处可以拖动窗口。先于栏里的控件注册，后添加的控件会覆盖它。
pub(super) fn drag_window(ui: &mut Ui, rect: Rect) {
    let drag = ui.interact(rect, ui.id().with("drag-window"), Sense::drag());
    // macOS 由平台层自己发起拖动（winit 的实现可能用错起点，见 `panel::start_drag`）。
    if drag.drag_started() && !native_panel::start_drag() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}
