//! 结果区工具条：类型筛选下拉 + 视图切换（参照 Everything 的类型过滤与「视图」菜单）。
//!
//! 两个下拉都是"按钮 + 浮层列表"：类型筛选决定查询条件（音频 / 文档 / 图片…），
//! 视图切换只影响绘制（超大图标 / 大图标 / 中等图标 / 详情，快捷键 ⌘⇧1…4）。
//! 浮层用 `egui::Area` 画在 Foreground 层（与历史、保存搜索下拉同一套外观），
//! 点空白处 / Esc / 选中一项后关闭。

use egui::{Align, Align2, FontId, Layout, Rect, Response, Sense, Stroke, Ui, Vec2};

use devtools_search_core::{FileKind, SortColumn, SortDir};
use devtools_ui::keys;

use super::icons::{self, Icon};
use super::theme::{self, Palette};
use super::ViewState;
use crate::application::service::Service;
use crate::settings::ResultView;

/// 工具条高度（与表头分开算：表头只在详情视图出现）。
pub const TOOLBAR_HEIGHT: f32 = 34.0;

/// 类型下拉的宽度。
const KIND_MENU_WIDTH: f32 = 156.0;
/// 视图菜单的宽度（要放得下快捷键提示）。
const VIEW_MENU_WIDTH: f32 = 210.0;
/// 排序下拉的宽度。
const SORT_MENU_WIDTH: f32 = 176.0;
/// 菜单项高度。
const ITEM_HEIGHT: f32 = 26.0;
/// 菜单项左侧圆点的位置（选中标记），文字从它右边开始。
const DOT_X: f32 = 13.0;
const TEXT_X: f32 = 25.0;

/// 工具条产生的动作（由模块转成命令）。
#[derive(Default)]
pub struct ToolbarAction {
    pub set_kind: Option<FileKind>,
    pub set_view: Option<ResultView>,
    /// 排序下拉里选的列与方向（和表头点击走同一条命令）。
    pub set_sort: Option<(SortColumn, SortDir)>,
}

pub fn show(ui: &mut Ui, view: &mut ViewState, service: &Service) -> ToolbarAction {
    let palette = theme::palette(ui);
    let mut action = ToolbarAction::default();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, TOOLBAR_HEIGHT), Sense::hover());

    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                egui::pos2(rect.left() + 10.0, rect.top() + 5.0),
                egui::pos2(rect.right() - 10.0, rect.bottom() - 5.0),
            ))
            .layout(Layout::left_to_right(Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = 6.0;

    // 左侧：类型筛选。图标用「盒子」表示类别集合。
    let kind = service.settings().ui.kind;
    let kind_button = dropdown_button(&mut bar, &palette, Some(Icon::Box), kind.label(), view.kind_open);
    let kind_rect = kind_button.rect;
    if kind_button.clicked() {
        view.kind_open = !view.kind_open;
        view.view_open = false;
    }

    // 右侧：视图切换 + 排序（排序紧挨着视图的左边）。
    let current_view = service.settings().ui.view;
    let sort = service.settings().ui.sort;
    let mut view_rect = Rect::NOTHING;
    let mut sort_rect = Rect::NOTHING;
    bar.with_layout(Layout::right_to_left(Align::Center), |ui| {
        // 右到左：先放的在最右边，所以先画视图、再画排序。
        let button = dropdown_button(ui, &palette, Some(Icon::List), current_view.label(), view.view_open);
        view_rect = button.rect;
        if button.clicked() {
            view.view_open = !view.view_open;
            view.kind_open = false;
            view.sort_open = false;
        }
        let sort_label = format!("{}{}", sort_column_label(sort.column), sort_arrow(sort.column, sort));
        let button = dropdown_button(ui, &palette, Some(Icon::Sort), &sort_label, view.sort_open);
        sort_rect = button.rect;
        if button.clicked() {
            view.sort_open = !view.sort_open;
            view.view_open = false;
            view.kind_open = false;
        }
    });

    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, palette.border.gamma_multiply(0.55)),
    );

    // 浮层：类型列表。
    if view.kind_open {
        let (picked, area) =
            list_menu(ui, &palette, "search-kind-menu", kind_rect, KIND_MENU_WIDTH, |ui| {
                let mut picked = None;
                for option in FileKind::ALL {
                    if menu_item(ui, &palette, option.label(), None, option == kind) {
                        picked = Some(option);
                    }
                }
                picked
            });
        if let Some(option) = picked {
            action.set_kind = Some(option);
            view.kind_open = false;
        } else if !stays_open(ui, kind_rect, area) {
            view.kind_open = false;
        }
    }

    // 浮层：视图菜单。
    if view.view_open {
        let (picked, area) =
            list_menu(ui, &palette, "search-view-menu", view_rect, VIEW_MENU_WIDTH, |ui| {
                let mut picked = None;
                for option in ResultView::ALL {
                    let shortcut = keys::shortcut(&[keys::COMMAND, keys::SHIFT, option.digit()]);
                    if menu_item(ui, &palette, option.label(), Some(&shortcut), option == current_view)
                    {
                        picked = Some(option);
                    }
                }
                picked
            });
        if let Some(option) = picked {
            action.set_view = Some(option);
            view.view_open = false;
        } else if !stays_open(ui, view_rect, area) {
            view.view_open = false;
        }
    }

    // 浮层：排序菜单。
    if view.sort_open {
        let (picked, area) = list_menu(ui, &palette, "search-sort-menu", sort_rect, SORT_MENU_WIDTH, |ui| {
            let mut picked = None;
            for column in SORT_COLUMNS {
                let selected = column == sort.column;
                let hint = sort_arrow(column, sort);
                if menu_item(ui, &palette, sort_column_label(column), Some(&hint), selected) {
                    picked = Some(column);
                }
            }
            picked
        });
        if let Some(column) = picked {
            // 点当前列切换升降序（路径列没有方向，等价于回到默认路径序）。
            let dir = if column == SortColumn::Path {
                SortDir::None
            } else if column == sort.column && !sort.descending {
                SortDir::Desc
            } else {
                SortDir::Asc
            };
            action.set_sort = Some((column, dir));
            view.sort_open = false;
        } else if !stays_open(ui, sort_rect, area) {
            view.sort_open = false;
        }
    }

    // 下拉打开时 Esc 先关下拉（不要落到结果区去清空搜索 / 收起窗口）。
    if (view.kind_open || view.view_open || view.sort_open)
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        view.kind_open = false;
        view.view_open = false;
        view.sort_open = false;
    }

    action
}

/// 排序下拉里列的列（顺序与表头一致）。
const SORT_COLUMNS: [SortColumn; 5] = [
    SortColumn::Name,
    SortColumn::Path,
    SortColumn::Size,
    SortColumn::Modified,
    SortColumn::Created,
];

/// 排序下拉里显示的名字。
fn sort_column_label(column: SortColumn) -> &'static str {
    match column {
        SortColumn::Name => "名称",
        SortColumn::Path => "路径",
        SortColumn::Size => "大小",
        SortColumn::Modified => "修改时间",
        SortColumn::Created => "创建时间",
    }
}

/// 方向标记：路径列是"默认顺序"，没有升降之分。
fn sort_arrow(column: SortColumn, sort: crate::settings::UiSort) -> &'static str {
    if column == SortColumn::Path || column != sort.column {
        ""
    } else if sort.descending {
        " ↓"
    } else {
        " ↑"
    }
}

/// 下拉按钮：可选图标 + 文字 + 下箭头；`open` 时用强调色底。
fn dropdown_button(
    ui: &mut Ui,
    palette: &Palette,
    icon: Option<Icon>,
    label: &str,
    open: bool,
) -> Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        FontId::proportional(12.5),
        palette.text,
    );
    let icon_part = if icon.is_some() { 18.0 } else { 0.0 };
    let width = galley.size().x + icon_part + 10.0 + 10.0 + 14.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::click());
    let fill = if open {
        palette.accent.gamma_multiply(0.16)
    } else if response.hovered() {
        palette.field
    } else {
        palette.card
    };
    ui.painter().add(theme::continuous_rect(
        rect,
        8.0,
        fill,
        Stroke::new(1.0, palette.card_stroke),
    ));
    let mut x = rect.left() + 9.0;
    if let Some(icon) = icon {
        icons::paint_centered(
            ui.painter(),
            Rect::from_center_size(egui::pos2(x + 6.0, rect.center().y), Vec2::splat(12.0)),
            12.0,
            icon,
            if open { palette.accent } else { palette.weak },
        );
        x += icon_part;
    }
    ui.painter().galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        palette.text,
    );
    icons::paint_centered(
        ui.painter(),
        Rect::from_center_size(
            egui::pos2(rect.right() - 11.0, rect.center().y),
            Vec2::splat(10.0),
        ),
        10.0,
        if open {
            Icon::ChevronUp
        } else {
            Icon::ChevronDown
        },
        palette.weak,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// 画一个浮层菜单：贴在 `anchor` 下边缘，返回 (选中的行, 菜单矩形)。
fn list_menu<R>(
    ui: &Ui,
    palette: &Palette,
    id: &str,
    anchor: Rect,
    width: f32,
    contents: impl FnOnce(&mut Ui) -> R,
) -> (R, Rect) {
    // 默认左对齐按钮；右侧放不下时改成右对齐（窗口变窄也不会伸到外面）。
    let screen = ui.ctx().content_rect();
    let x = if anchor.left() + width + 8.0 <= screen.right() {
        anchor.left()
    } else {
        anchor.right() - width
    };
    let pos = egui::pos2(x.max(screen.left() + 8.0), anchor.bottom() + 4.0);
    let mut rect = Rect::NOTHING;
    let mut value: Option<R> = None;
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(if palette.dark {
                    egui::Color32::from_rgb(44, 44, 48)
                } else {
                    egui::Color32::WHITE
                })
                .corner_radius(f32::from(theme::MENU_RADIUS))
                .stroke(Stroke::new(1.0, palette.border))
                .shadow(egui::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(if palette.dark { 110 } else { 40 }),
                })
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(width - 12.0);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    value = Some(contents(ui));
                });
            rect = ui.min_rect();
        });
    (value.expect("浮层内容总会绘制一次"), rect)
}

/// 一行菜单项：左侧圆点（选中时强调色）+ 文字 + 右侧快捷键；返回是否点击。
fn menu_item(
    ui: &mut Ui,
    palette: &Palette,
    label: &str,
    shortcut: Option<&str>,
    selected: bool,
) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ITEM_HEIGHT), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 6.0, palette.field);
    }
    if selected {
        ui.painter().circle_filled(
            egui::pos2(rect.left() + DOT_X, rect.center().y),
            3.0,
            palette.accent,
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + TEXT_X, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        if selected { palette.accent } else { palette.text },
    );
    if let Some(shortcut) = shortcut {
        ui.painter().text(
            egui::pos2(rect.right() - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            shortcut,
            FontId::proportional(theme::TEXT_HINT),
            palette.faint,
        );
    }
    response.clicked()
}

/// 浮层是否还应该保持打开：点在下拉按钮或菜单之外就关掉。
fn stays_open(ui: &Ui, button: Rect, area: Rect) -> bool {
    if ui.input(|i| i.pointer.any_pressed()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            return area.contains(pos) || button.contains(pos);
        }
    }
    true
}
