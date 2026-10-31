//! 状态栏：索引状态点、计数、耗时、快捷键提示与窗口模式（§5.1、§5.3）。

use devtools_search_core::volumes::RootState;
use devtools_ui::keys;
use egui::{Align, FontId, Layout, RichText, Sense, Stroke, Ui, Vec2};

use super::icons::{self, Icon};
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

    // 左侧：有选中项时显示它的完整路径（中间省略，悬停看全文），否则显示索引状态。
    // 路径优先，因为"这个东西到底在哪"是选完一项后最常问的问题。
    let selected = view
        .cache
        .get(view.cursor as u32)
        .filter(|row| !row.missing)
        .cloned();
    match &selected {
        Some(row) => {
            let (icon_rect, _) = bar.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
            icons::paint_centered(
                bar.painter(),
                icon_rect,
                11.0,
                if row.kind == devtools_search_core::NodeKind::Dir {
                    Icon::Folder
                } else {
                    Icon::Document
                },
                palette.weak,
            );
            bar.add_space(3.0);
            // 右侧还要放模式胶囊、计数与快捷键提示，给路径留出剩下的宽度。
            let width = (bar.available_width() - 390.0).max(120.0);
            let text = theme::middle_ellipsis(
                &bar,
                &row.path,
                FontId::proportional(theme::TEXT_HINT),
                width,
            );
            bar.label(
                RichText::new(text)
                    .size(theme::TEXT_HINT)
                    .color(palette.secondary),
            )
            .on_hover_text(&row.path);
            // 文件大小跟在路径后面（目录没有大小，显示破折号）。
            let size = match (row.kind, row.size) {
                (devtools_search_core::NodeKind::File, Some(bytes)) => theme::byte_size(bytes),
                _ => "—".to_string(),
            };
            bar.label(
                RichText::new(format!("· {size}"))
                    .size(theme::TEXT_HINT)
                    .color(palette.weak),
            );
        }
        None => {
            let (text, dot) = aggregate(view, service, &palette);
            theme::status_dot(&mut bar, dot);
            bar.label(
                RichText::new(text)
                    .size(theme::TEXT_HINT)
                    .color(palette.weak),
            );
        }
    }
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
        // 窗口模式：图标 + 文字胶囊。点击循环 浮动 → 固定 → 普通。
        let mode = service.window_policy().mode;
        let hint = match mode {
            crate::SearchWindowMode::Floating => "浮动窗口：失焦收起",
            crate::SearchWindowMode::Pinned => "固定窗口：失焦不收起，始终置顶",
            crate::SearchWindowMode::Normal => "普通窗口：带标题栏，不置顶",
        };
        if theme::pill_button(
            ui,
            &palette,
            mode.label(),
            mode != crate::SearchWindowMode::Floating,
        )
        .on_hover_text(hint)
        .clicked()
        {
            action.toggle_mode = true;
        }
        let _ = Icon::Pin;

        let selected = view.selected_count();
        if selected > 1 {
            ui.label(
                RichText::new(format!("已选 {} 项", theme::thousands(selected)))
                    .size(theme::TEXT_HINT)
                    .color(palette.accent),
            );
        }
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
            // 按键的写法跟平台走：macOS 显示 ↵ / ⌘R，Windows 显示 Enter / Ctrl+R。
            ui.label(
                RichText::new("打开")
                    .size(theme::TEXT_HINT)
                    .color(palette.faint),
            );
            theme::keycaps(ui, &[keys::RETURN], &palette);
            ui.add_space(2.0);
            ui.label(
                RichText::new("显示")
                    .size(theme::TEXT_HINT)
                    .color(palette.faint),
            );
            theme::keycaps(ui, &[keys::COMMAND, "R"], &palette);
        }
    });

    let _ = (
        Sense::hover(),
        FontId::proportional(11.0),
        Layout::left_to_right(Align::Center),
    );
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
