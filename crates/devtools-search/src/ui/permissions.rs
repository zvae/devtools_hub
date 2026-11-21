//! 首次使用的权限 / 同意引导（§5.3）。
//!
//! macOS 读取其他应用数据可能需要在"系统设置 → 隐私与安全性 → 完全磁盘访问权限"里授权；
//! 这里解释原因并提供两个选择：现在建立索引 / 稍后（仍可搜索空索引）。

use egui::{Align, Layout, Rect, RichText, Sense, Stroke, Ui, Vec2};

use super::icons::{self, Icon};
use super::theme;

/// 引导页的选择。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermAction {
    /// 现在建立索引。
    Begin,
    /// 稍后（按需在首次搜索时建立）。
    Later,
}

pub fn show(ui: &mut Ui, read_only: bool) -> Option<PermAction> {
    let palette = theme::palette(ui);
    let mut choice = None;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), ui.available_height().max(240.0)),
        Sense::hover(),
    );
    let card = Rect::from_center_size(
        rect.center(),
        Vec2::new(460.0_f32.min(rect.width() - 32.0), 348.0),
    );
    ui.painter().add(theme::continuous_rect(
        card,
        f32::from(theme::CARD_RADIUS),
        palette.card,
        Stroke::new(1.0, palette.card_stroke),
    ));
    // 顶部图标：圆形底 + 放大镜。
    let badge = Rect::from_center_size(
        egui::pos2(card.center().x, card.top() + 52.0),
        Vec2::splat(58.0),
    );
    ui.painter()
        .circle_filled(badge.center(), 29.0, palette.accent.gamma_multiply(0.12));
    icons::paint_centered(ui.painter(), badge, 25.0, Icon::Search, palette.accent);

    let content = Rect::from_min_max(
        egui::pos2(card.left() + 30.0, badge.bottom() + 14.0),
        egui::pos2(card.right() - 30.0, card.bottom() - 16.0),
    );
    let mut area = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content)
            .layout(Layout::top_down(Align::Center)),
    );
    area.spacing_mut().item_spacing = Vec2::new(0.0, 7.0);
    area.label(
        RichText::new("建立文件索引")
            .font(theme::title_font(&area, 17.0))
            .color(palette.text),
    );
    area.label(
        RichText::new("在本地为文件建立索引，之后可以按文件名、路径、类型、大小即时搜索。")
            .size(theme::TEXT_HINT)
            .line_height(Some(18.0))
            .color(palette.weak),
    );
    area.add_space(2.0);
    for line in [
        "索引只保存在本机，不上传、不读取文件内容。",
        "首次建立需要几分钟到十几分钟，期间可以边建边搜。",
        cfg_hint(),
    ] {
        if line.is_empty() {
            continue;
        }
        // 每条说明整体居中，圆点用强调色。
        area.label(
            RichText::new(format!("•  {line}"))
                .size(theme::TEXT_HINT)
                .line_height(Some(17.0))
                .color(palette.weak),
        );
    }
    if read_only {
        area.label(
            RichText::new("设置文件为只读，本次运行不会保存改动")
                .size(theme::TEXT_HINT)
                .color(palette.danger),
        );
    }
    area.add_space(8.0);

    // 按钮按实际宽度居中。
    let primary = "开始建立索引";
    let secondary = "稍后";
    let primary_w = ui
        .painter()
        .layout_no_wrap(primary.to_string(), egui::FontId::proportional(12.5), palette.text)
        .size()
        .x
        + 40.0;
    let secondary_w = ui
        .painter()
        .layout_no_wrap(
            secondary.to_string(),
            egui::FontId::proportional(12.5),
            palette.text,
        )
        .size()
        .x
        + 20.0;
    let total = primary_w + secondary_w + 10.0;
    let row_width = area.available_width();
    area.horizontal(|ui| {
        ui.add_space(((row_width - total) / 2.0).max(0.0));
        if theme::primary_button(ui, Some(Icon::Search), primary, &palette).clicked() {
            choice = Some(PermAction::Begin);
        }
        if theme::labeled_button(ui, None, secondary, true, &palette).clicked() {
            choice = Some(PermAction::Later);
        }
    });
    area.add_space(2.0);
    area.label(
        RichText::new("稍后也可以：输入关键词时会自动开始建立索引")
            .size(theme::TEXT_HINT)
            .color(palette.faint),
    );
    choice
}

/// 平台相关的权限提示。
fn cfg_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "部分目录没有结果时，请在“系统设置 → 隐私与安全性 → 完全磁盘访问权限”中勾选本应用。"
    } else {
        "系统盘与数据盘会分别建立索引；加密目录可能需要管理员权限。"
    }
}
