//! 工具共用的无平台界面适配；原生拖动交给宿主在绘制后执行。

use devtools_module_api::ToolEffect;
pub use devtools_ui::{fonts, icons, keys, style, widgets};

pub fn drag_window(ui: &mut egui::Ui, rect: egui::Rect, effects: &mut Vec<ToolEffect>) {
    if ui
        .interact(rect, ui.id().with("drag-window"), egui::Sense::drag())
        .drag_started()
    {
        effects.push(ToolEffect::StartDrag);
    }
}

macro_rules! tool_helpers {
    ($tool:ty) => {
        impl $tool {
            fn palette(&self, _: &egui::Context) -> devtools_ui::style::Palette {
                self.palette
            }
            fn show_toast(&mut self, text: impl Into<String>) {
                self.effects
                    .push(devtools_module_api::ToolEffect::Toast(text.into()));
            }
            fn hide_panel(&mut self, _: bool) {
                self.effects.push(devtools_module_api::ToolEffect::Close);
            }
        }
    };
}
pub(crate) use tool_helpers;
