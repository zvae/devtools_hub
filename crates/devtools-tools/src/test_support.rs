//! 工具离屏测试只记录宿主请求，不访问真实系统剪贴板或原生窗口。

use devtools_module_api::{ToolEffect, ToolModule};
use egui_kittest::Harness;

pub struct Fixture<T> {
    installed: bool,
    pub tool: T,
    pub toast: Option<String>,
    pub copied: Option<(String, bool)>,
    pub closed: bool,
}

pub fn harness<T: ToolModule + 'static>(
    tool: T,
    theme: egui::Theme,
    size: egui::Vec2,
) -> Harness<'static, Fixture<T>> {
    let mut harness = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(2.0)
        .wgpu()
        .build_ui_state(
            |ui, state: &mut Fixture<T>| {
                if !state.installed {
                    devtools_ui::style::install(ui.ctx());
                    state.installed = true;
                    return;
                }
                let palette = devtools_ui::style::Palette::new(ui.visuals().dark_mode);
                for effect in state.tool.render(ui, &palette) {
                    match effect {
                        ToolEffect::Toast(text) => state.toast = Some(text),
                        ToolEffect::Copy { text, sensitive } => {
                            state.copied = Some((text, sensitive))
                        }
                        ToolEffect::Close => state.closed = true,
                        ToolEffect::StartDrag => {}
                    }
                }
            },
            Fixture {
                installed: false,
                tool,
                toast: None,
                copied: None,
                closed: false,
            },
        );
    devtools_ui::style::install(&harness.ctx);
    harness.ctx.set_theme(theme);
    harness
        .ctx
        .all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
    harness.run_steps(3);
    harness
}

pub fn save<T>(harness: &mut Harness<'_, Fixture<T>>, name: &str) {
    let dir = std::env::var_os("UI_SNAPSHOT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-snapshots")
        });
    std::fs::create_dir_all(&dir).unwrap();
    harness
        .render()
        .expect("工具离屏渲染失败")
        .save(dir.join(format!("{name}.png")))
        .unwrap();
}
