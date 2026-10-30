//! 设置面板：卡片分组 + 开关/胶囊按钮，草稿与应用分离（§5.5、§6）。
//!
//! 视觉与宿主设置页一致：分组标题 + 15pt 圆角卡片、iOS 风格开关、主/次按钮。
//! 编辑只改本地草稿，点"保存并应用"才经 Service 校验、原子保存并生效。

use egui::{Align, Layout, Rect, RichText, Sense, ScrollArea, Stroke, Ui, Vec2};

use super::icons::{self, Icon};
use super::{theme, ViewState};
use crate::application::service::Service;
use crate::settings::SearchWindowMode;

/// 设置面板的动作。
#[derive(Default)]
pub struct SettingsOutcome {
    pub apply: bool,
    pub back: bool,
    pub pick_folder: bool,
    pub rebuild_all: bool,
    pub clear_cache: bool,
    pub toggle_pause: bool,
    pub clear_history: bool,
}

pub fn show(ui: &mut Ui, view: &mut ViewState, service: &Service) -> SettingsOutcome {
    let palette = theme::palette(ui);
    let mut outcome = SettingsOutcome::default();
    let mut draft = view
        .draft
        .clone()
        .unwrap_or_else(|| service.settings().clone());
    let mut new_exclude = std::mem::take(&mut view.new_exclude);
    let error = view.settings_error.clone();
    let read_only = view.read_only;

    // 整页左右留白：卡片不贴窗口边缘（与宿主设置页的边距一致）。
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 18,
            right: 18,
            top: 10,
            bottom: 12,
        })
        .show(ui, |ui| {
    // 标题栏：返回 + 标题 + 只读提示。
    ui.horizontal(|ui| {
        if theme::icon_button(ui, Icon::Back, false, &palette)
            .on_hover_text("返回结果列表（Esc）")
            .clicked()
        {
            outcome.back = true;
        }
        ui.add_space(2.0);
        ui.label(
            RichText::new("搜索设置")
                .font(theme::title_font(ui, theme::TEXT_TITLE))
                .color(palette.text),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if read_only {
                ui.label(
                    RichText::new("只读")
                        .size(theme::TEXT_HINT)
                        .color(palette.danger),
                );
            }
        });
    });
    ui.add_space(2.0);

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // 索引根。
            theme::section(ui, &palette, "索引根", |ui| {
                let mut remove = None;
                for (index, root) in draft.roots.iter_mut().enumerate() {
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 32.0),
                        Sense::hover(),
                    );
                    let mut row = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(Layout::left_to_right(Align::Center)),
                    );
                    row.spacing_mut().item_spacing.x = 8.0;
                    theme::toggle(&mut row, &palette, &mut root.enabled);
                    row.label(
                        RichText::new(root.location.fallback_absolute_path.to_string_lossy())
                            .size(12.5)
                            .color(palette.text),
                    );
                    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::icon_button(ui, Icon::Close, false, &palette)
                            .on_hover_text("移除此索引根")
                            .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                }
                if let Some(index) = remove {
                    draft.roots.remove(index);
                }
                ui.horizontal(|ui| {
                    if theme::labeled_button(
                        ui,
                        Some(Icon::Plus),
                        "添加目录…",
                        true,
                        &palette,
                    )
                    .clicked()
                    {
                        outcome.pick_folder = true;
                    }
                    ui.label(
                        RichText::new("新加的目录在应用后开始建立索引")
                            .size(theme::TEXT_HINT)
                            .color(palette.faint),
                    );
                });
                ui.add_space(2.0);
            });

            // 排除规则。
            theme::section(ui, &palette, "排除规则", |ui| {
                theme::hint(
                    ui,
                    &palette,
                    "只写一段（node_modules/、Library/、*.log）按任意层级匹配，结尾 / 表示只匹配目录；\
                     从根目录写完整路径（AppData/Local/Temp/，或前面加 /）才按根相对匹配。",
                );
                let mut remove = None;
                for (index, pattern) in draft.exclude_patterns.iter().enumerate() {
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 26.0),
                        Sense::hover(),
                    );
                    if response.hovered() {
                        ui.painter().rect_filled(rect, 6.0, palette.field);
                    }
                    ui.painter().text(
                        egui::pos2(rect.left() + 6.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        pattern,
                        egui::FontId::monospace(12.0),
                        palette.text,
                    );
                    let mut close_ui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
                        egui::pos2(rect.right() - 26.0, rect.top()),
                        Vec2::new(26.0, 26.0),
                    )));
                    if theme::icon_button(&mut close_ui, Icon::Close, false, &palette)
                        .on_hover_text("移除规则")
                        .clicked()
                    {
                        remove = Some(index);
                    }
                }
                if let Some(index) = remove {
                    draft.exclude_patterns.remove(index);
                }
                ui.horizontal(|ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut new_exclude)
                            .hint_text("例如 node_modules/、/Library/、**/build/**")
                            .desired_width(260.0)
                            .frame(
                                egui::Frame::new()
                                    .fill(palette.card)
                                    .stroke(Stroke::new(1.0, palette.card_stroke))
                                    .corner_radius(6.0)
                                    .inner_margin(egui::Margin::symmetric(8, 4)),
                            ),
                    );
                    let submit =
                        response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (theme::labeled_button(ui, Some(Icon::Plus), "添加", true, &palette)
                        .clicked()
                        || submit)
                        && !new_exclude.trim().is_empty()
                    {
                        draft.exclude_patterns.push(new_exclude.trim().to_string());
                        new_exclude.clear();
                    }
                });
                ui.add_space(2.0);
            });

            // 索引行为。
            theme::section(ui, &palette, "索引行为", |ui| {
                theme::row(ui, &palette, "惰性启动（首次搜索时再建立索引）", |ui| {
                    theme::toggle(ui, &palette, &mut draft.lazy_start);
                });
                theme::divider(ui, &palette);
                theme::row(ui, &palette, "搜索页隐藏后继续建立索引", |ui| {
                    theme::toggle(ui, &palette, &mut draft.continue_when_hidden);
                });
                theme::divider(ui, &palette);
                theme::row(ui, &palette, "扫描 .app 包内部", |ui| {
                    theme::toggle(ui, &palette, &mut draft.scan_app_bundles);
                });
                theme::divider(ui, &palette);
                theme::row(ui, &palette, "保存索引快照（下次启动更快）", |ui| {
                    theme::toggle(ui, &palette, &mut draft.persist_index);
                });
                theme::divider(ui, &palette);
                theme::row(ui, &palette, "实时监听文件变化", |ui| {
                    theme::toggle(ui, &palette, &mut draft.watch);
                });
                theme::hint(
                    ui,
                    &palette,
                    "开启后新增、删除、改名会即时反映到结果里；监听不可用时仍可手动“重建全部”。",
                );
                ui.add_space(2.0);
            });

            // 运行控制。
            theme::section(ui, &palette, "运行控制", |ui| {
                ui.horizontal(|ui| {
                    let label = if draft.index_paused { "恢复索引" } else { "暂停索引" };
                    let icon = if draft.index_paused { Icon::Play } else { Icon::Pause };
                    if theme::labeled_button(ui, Some(icon), label, true, &palette).clicked() {
                        outcome.toggle_pause = true;
                    }
                    if theme::labeled_button(ui, None, "重建全部", true, &palette).clicked() {
                        outcome.rebuild_all = true;
                    }
                    if theme::labeled_button(ui, Some(Icon::Trash), "清理缓存", true, &palette)
                        .clicked()
                    {
                        outcome.clear_cache = true;
                    }
                });
                for root in service.roots() {
                    ui.horizontal(|ui| {
                        ui.add_space(2.0);
                        let color = match root.state {
                            devtools_search_core::volumes::RootState::Ready => palette.ok,
                            devtools_search_core::volumes::RootState::Scanning
                            | devtools_search_core::volumes::RootState::PendingValidation
                            | devtools_search_core::volumes::RootState::Verifying => palette.accent,
                            devtools_search_core::volumes::RootState::Error(_) => palette.danger,
                            _ => palette.faint,
                        };
                        theme::status_dot(ui, color);
                        ui.label(
                            RichText::new(format!(
                                "{}  ·  {}  ·  {} 个文件",
                                root.path.display(),
                                root.state.label(),
                                theme::thousands(root.files as usize)
                            ))
                            .size(theme::TEXT_HINT)
                            .color(palette.faint),
                        );
                    });
                }
                ui.add_space(2.0);
            });

            // 搜索行为。
            theme::section(ui, &palette, "搜索行为", |ui| {
                theme::row(ui, &palette, "默认区分大小写（Aa）", |ui| {
                    theme::toggle(ui, &palette, &mut draft.default_case_sensitive);
                });
                theme::divider(ui, &palette);
                theme::row(ui, &palette, "空查询时列出全部文件", |ui| {
                    theme::toggle(ui, &palette, &mut draft.show_all_on_empty);
                });
                theme::divider(ui, &palette);
                theme::row(ui, &palette, "高亮文件名中的匹配", |ui| {
                    theme::toggle(ui, &palette, &mut draft.highlight);
                });
                theme::divider(ui, &palette);
                theme::row(ui, &palette, "输入防抖", |ui| {
                    ui.spacing_mut().slider_width = 160.0;
                    ui.add(
                        egui::Slider::new(&mut draft.debounce_ms, 1..=500)
                            .suffix(" ms")
                            .trailing_fill(true),
                    );
                });
                theme::divider(ui, &palette);
                theme::row(ui, &palette, "结果上限", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut draft.max_results)
                            .range(0..=100_000)
                            .speed(10),
                    );
                    ui.label(
                        RichText::new("0 表示不限")
                            .size(theme::TEXT_HINT)
                            .color(palette.faint),
                    );
                });
                ui.add_space(2.0);
            });

            // 搜索库。
            theme::section(ui, &palette, "搜索库", |ui| {
                theme::row(ui, &palette, "记录搜索历史", |ui| {
                    theme::toggle(ui, &palette, &mut draft.history_enabled);
                });
                theme::divider(ui, &palette);
                theme::row(ui, &palette, "最多保留", |ui| {
                    ui.label(RichText::new("条").size(theme::TEXT_HINT).color(palette.faint));
                    ui.add(
                        egui::DragValue::new(&mut draft.history_limit)
                            .range(1..=10_000)
                            .speed(1),
                    );
                    if theme::labeled_button(ui, Some(Icon::Trash), "清空", true, &palette)
                        .clicked()
                    {
                        outcome.clear_history = true;
                    }
                });
                ui.add_space(2.0);
            });

            // 界面。
            theme::section(ui, &palette, "界面", |ui| {
                theme::row(ui, &palette, "窗口模式", |ui| {
                    let floating =
                        theme::pill_button(
                            ui,
                            &palette,
                            "浮动（失焦收起）",
                            draft.ui.window.mode == SearchWindowMode::Floating,
                        );
                    if floating.clicked() {
                        draft.ui.window.mode = SearchWindowMode::Floating;
                    }
                    let pinned = theme::pill_button(
                        ui,
                        &palette,
                        "固定（失焦保留）",
                        draft.ui.window.mode == SearchWindowMode::Pinned,
                    );
                    if pinned.clicked() {
                        draft.ui.window.mode = SearchWindowMode::Pinned;
                    }
                });
                theme::divider(ui, &palette);
                theme::row(ui, &palette, "列宽与排序", |ui| {
                    if theme::labeled_button(ui, None, "还原默认", true, &palette).clicked() {
                        draft.ui.columns = Default::default();
                        draft.ui.sort = Default::default();
                    }
                });
                theme::hint(ui, &palette, "列宽可直接在表头边界拖动调整");
                ui.add_space(2.0);
            });

            ui.add_space(10.0);
            if let Some(error) = &error {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    icons::paint_centered(
                        ui.painter(),
                        Rect::from_min_size(ui.cursor().min + Vec2::new(0.0, 4.0), Vec2::splat(12.0)),
                        12.0,
                        Icon::Warning,
                        palette.danger,
                    );
                    ui.add_space(16.0);
                    ui.label(
                        RichText::new(error)
                            .size(theme::TEXT_HINT)
                            .color(palette.danger),
                    );
                });
            }
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.add_enabled_ui(!read_only, |ui| {
                    if theme::primary_button(ui, Some(Icon::Check), "保存并应用", &palette)
                        .clicked()
                    {
                        outcome.apply = true;
                    }
                });
                if theme::labeled_button(ui, None, "还原", true, &palette).clicked() {
                    draft = service.settings().clone();
                }
                ui.label(
                    RichText::new("关闭面板不会保存改动")
                        .size(theme::TEXT_HINT)
                        .color(palette.faint),
                );
            });
            ui.add_space(10.0);
        });

        });

    view.draft = Some(draft);
    view.new_exclude = new_exclude;
    // Esc 返回。
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        outcome.back = true;
    }
    let _ = Stroke::NONE;
    outcome
}
