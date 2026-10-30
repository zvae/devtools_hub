//! 状态栏：索引状态点、计数、耗时、快捷键提示与窗口模式（§5.1、§5.3）。

use egui::{Align, FontId, Layout, RichText, Sense, Stroke, Ui, Vec2};
use devtools_search_core::volumes::RootState;

use super::icons::Icon;
use super::{theme, ViewState};
use crate::application::service::{Lifecycle, Service};

/// 状态栏动作。
#[derive(Default)]
pub struct StatusAction {
    pub toggle_mode: bool,
}

pub fn show(ui: &mut Ui, view: &mut ViewState, service: &Service) -> StatusAction {
    let palette = theme::palette(ui);
    let mut action = StatusAction::default();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, theme::STATUS_HEIGHT), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0, palette.border.gamma_multiply(0.55)),
    );
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(12.0, 2.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    bar.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);

    // 左侧：状态点 + 文字。
    let (text, dot) = aggregate(view, service, &palette);
    theme::status_dot(&mut bar, dot);
    bar.label(
        RichText::new(text)
            .size(theme::TEXT_HINT)
            .color(palette.weak),
    );
    if let Some(error) = &view.error {
        bar.label(
            RichText::new(error)
                .size(theme::TEXT_HINT)
                .color(palette.danger),
        );
    } else if let Some(notice) = &view.notice {
        bar.label(
            RichText::new(notice)
                .size(theme::TEXT_HINT)
                .color(palette.warn),
        );
    }

    // 右侧：模式切换、结果计数、快捷键提示。
    bar.with_layout(Layout::right_to_left(Align::Center), |ui| {
        // 窗口模式：图标 + 文字胶囊。
        let pinned = service.window_policy().mode == crate::SearchWindowMode::Pinned;
        let label = if pinned { "固定" } else { "浮动" };
        if theme::pill_button(ui, &palette, label, pinned)
            .on_hover_text(if pinned {
                "固定窗口：失焦不收起"
            } else {
                "浮动窗口：失焦收起"
            })
            .clicked()
        {
            action.toggle_mode = true;
        }
        let _ = Icon::Pin;

        if view.total > 0 {
            ui.label(
                RichText::new(format!(
                    "{} 个结果 · {} ms",
                    theme::thousands(view.total),
                    view.took_ms
                ))
                .size(theme::TEXT_HINT)
                .color(palette.weak),
            );
        } else if view.searching {
            ui.label(
                RichText::new("搜索中…")
                    .size(theme::TEXT_HINT)
                    .color(palette.weak),
            );
        }
        if service.results_len() > 0 {
            // 右到左布局：先放文字、再放键帽，视觉上才是「键帽 文字」。
            ui.label(
                RichText::new("打开")
                    .size(theme::TEXT_HINT)
                    .color(palette.faint),
            );
            theme::keycaps(ui, &palette, &["↵"]);
            ui.add_space(2.0);
            ui.label(
                RichText::new("显示")
                    .size(theme::TEXT_HINT)
                    .color(palette.faint),
            );
            theme::keycaps(ui, &palette, &["⌘R"]);
        }
    });

    let _ = (Sense::hover(), FontId::proportional(11.0), Layout::left_to_right(Align::Center));
    action
}

/// 聚合根状态 → (文字, 颜色)。
fn aggregate(
    view: &ViewState,
    service: &Service,
    palette: &theme::Palette,
) -> (String, egui::Color32) {
    if service.read_only() {
        return ("只读设置".into(), palette.faint);
    }
    if view.searching {
        return ("搜索中…".into(), palette.accent);
    }
    if view.roots.is_empty() {
        return match view.lifecycle {
            Lifecycle::AwaitingConsent => ("等待授权".into(), palette.faint),
            _ => ("没有索引根".into(), palette.faint),
        };
    }
    let mut files = 0u32;
    let mut dirs = 0u32;
    let mut scanning = false;
    let mut verifying = false;
    let mut error = false;
    let mut offline = false;
    let mut paused = false;
    let mut snapshot = false;
    for root in &view.roots {
        files += root.files;
        dirs += root.dirs;
        snapshot |= root.from_snapshot;
        match root.state {
            RootState::Scanning => scanning = true,
            RootState::PendingValidation | RootState::Verifying => verifying = true,
            RootState::Error(_) => error = true,
            RootState::Offline => offline = true,
            RootState::Paused | RootState::Dirty => paused = true,
            _ => {}
        }
    }
    if scanning {
        return ("正在建立索引…".into(), palette.accent);
    }
    if verifying {
        return ("验证索引…".into(), palette.accent);
    }
    if error {
        return ("索引出错".into(), palette.danger);
    }
    if offline {
        return ("卷未挂载".into(), palette.faint);
    }
    if paused {
        return ("索引已暂停".into(), palette.faint);
    }
    let suffix = if snapshot { "（快照）" } else { "" };
    (
        format!(
            "就绪{suffix} · {} 个文件 · {} 个目录",
            theme::thousands(files as usize),
            theme::thousands(dirs as usize)
        ),
        palette.ok,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_thousands() {
        assert_eq!(theme::thousands(0), "0");
        assert_eq!(theme::thousands(1000), "1,000");
        assert_eq!(theme::thousands(1234567), "1,234,567");
    }
}
