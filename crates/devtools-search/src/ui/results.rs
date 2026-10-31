//! 结果区：详情表格与图标网格两种视图、键盘操作与右键菜单（§5.1、§5.2）。
//!
//! 两种视图共用同一份结果集（`RowCache` 里按下标存的行）与同一套选中状态
//! （`cursor` / `anchor` 都是结果下标），所以切换视图不会丢选中、也不会重新查一次；
//! 每种视图有独立的滚动位置（`ScrollArea` 各自记住偏移）。

use std::path::PathBuf;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, CursorIcon, FontId, Key, Modifiers, Rect, Sense, Stroke, Ui, Vec2};
use devtools_search_core::{FileKind, NodeKind, SortColumn};

use super::icons::{self, Icon};
use super::theme::{self, Palette};
use super::ViewState;
use crate::application::protocol::RowData;
use crate::application::service::{Lifecycle, Service};
use crate::settings::{ColumnWidths, ResultView};

/// 图标网格里卡片之间的间距。
const GRID_GAP: f32 = 10.0;
/// 网格左右两侧与内容边缘的距离（第一列左边、最后一列右边也留出呼吸感）。
const GRID_SIDE_PAD: f32 = 10.0;
/// 卡片内图标上方留白。
const GRID_ICON_TOP: f32 = 6.0;
/// 卡片底部给文件名留的高度（两行）。
const GRID_NAME_HEIGHT: f32 = 34.0;

/// 表格产生的动作（由模块转成命令）。
#[derive(Default)]
pub struct ResultsAction {
    pub close: bool,
    pub requests: Vec<(u32, u32)>,
    pub open: Option<PathBuf>,
    pub reveal: Option<PathBuf>,
    pub copy: Option<String>,
    /// 表头点击：切换排序。
    pub sort: Option<(SortColumn, devtools_search_core::SortDir)>,
    /// 列宽拖动结束（持久化）。
    pub column_resize: Option<ColumnWidths>,
    /// 空态里的"立即建立索引"。
    pub start_indexing: bool,
    /// 工具条：切换类型筛选。
    pub set_kind: Option<FileKind>,
    /// 工具条：切换结果区视图。
    pub set_view: Option<ResultView>,
    /// 右键点了某一项：交给系统右键菜单（Windows）。
    pub system_menu: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    Name,
    Path,
    Size,
    Modified,
    Created,
}

impl Column {
    const ALL: [Column; 5] = [
        Column::Name,
        Column::Path,
        Column::Size,
        Column::Modified,
        Column::Created,
    ];

    fn sort_column(self) -> SortColumn {
        match self {
            Column::Name => SortColumn::Name,
            Column::Path => SortColumn::Path,
            Column::Size => SortColumn::Size,
            Column::Modified => SortColumn::Modified,
            Column::Created => SortColumn::Created,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Column::Name => "文件名",
            Column::Path => "路径",
            Column::Size => "大小",
            Column::Modified => "修改时间",
            Column::Created => "创建时间",
        }
    }
}

/// 图标网格的度量（三种图标视图各一套）。
#[derive(Clone, Copy)]
struct GridStyle {
    cell_w: f32,
    cell_h: f32,
    icon: f32,
}

impl GridStyle {
    fn for_view(view: ResultView) -> Self {
        match view {
            ResultView::ExtraLarge => Self {
                cell_w: 140.0,
                cell_h: 150.0,
                icon: 96.0,
            },
            ResultView::Large => Self {
                cell_w: 112.0,
                cell_h: 118.0,
                icon: 72.0,
            },
            ResultView::Medium => Self {
                cell_w: 88.0,
                cell_h: 96.0,
                icon: 52.0,
            },
            // 详情视图不用这些度量；给一个安全值（键盘换算行高时会读到）。
            ResultView::Detail => Self {
                cell_w: 0.0,
                cell_h: theme::ROW_HEIGHT,
                icon: 0.0,
            },
        }
    }
}

pub fn show(
    ui: &mut Ui,
    view: &mut ViewState,
    service: &Service,
    host: &std::sync::Arc<dyn crate::HostServices>,
) -> ResultsAction {
    let mut action = ResultsAction::default();
    ui.spacing_mut().item_spacing = Vec2::ZERO;

    // 工具条与结果区上下紧贴（类型筛选 + 视图切换）。
    let toolbar = super::toolbar::show(ui, view, service);
    action.set_kind = toolbar.set_kind;
    action.set_view = toolbar.set_view;
    // 工具条的排序下拉和表头点击走同一条命令（表头改完，下拉的显示自动跟着变）。
    action.sort = toolbar.set_sort;

    match service.settings().ui.view {
        ResultView::Detail => detail_view(ui, view, service, host, &mut action),
        mode => grid_view(ui, view, service, host, mode, &mut action),
    }
    action
}

/// 详情视图：五列表头 + 卡片式行。
fn detail_view(
    ui: &mut Ui,
    view: &mut ViewState,
    service: &Service,
    host: &std::sync::Arc<dyn crate::HostServices>,
    action: &mut ResultsAction,
) {
    let palette = theme::palette(ui);
    let available = ui.available_width().max(360.0);
    let widths = column_widths(view, available);

    // 表头始终显示（没有结果时不抽走表头和底部状态栏，避免搜索过程中布局闪动）。
    // 没有行时也一直显示空态（搜索中换成"搜索中…"），不要在"空白表格"和"空态"之间来回切换。
    let rows = service.results_len();
    let empty = rows == 0 && view.total == 0;

    // 表头。
    let (head_rect, _) = ui.allocate_exact_size(
        Vec2::new(available, theme::TABLE_HEADER_HEIGHT),
        Sense::hover(),
    );
    let sort = (
        service.settings().ui.sort.column,
        service.settings().ui.sort.descending,
    );
    let mut x = head_rect.left() + 6.0;
    for (index, column) in Column::ALL.iter().enumerate() {
        let width = widths[index];
        let cell = Rect::from_min_size(
            egui::pos2(x, head_rect.top()),
            Vec2::new(width, head_rect.height()),
        );
        let sorted = sort.0 == column.sort_column();
        let response = ui.interact(cell, ui.id().with(("search-head", index)), Sense::click());
        let color = if sorted {
            palette.text
        } else {
            palette.secondary
        };
        let label_galley = ui.painter().layout_no_wrap(
            column.label().to_string(),
            theme::title_font(ui, 12.0),
            color,
        );
        let text_x = match column {
            Column::Size => cell.right() - 10.0 - label_galley.size().x,
            // 与行内"图标 + 名称"的文字起点对齐。
            Column::Name => cell.left() + 32.0,
            _ => cell.left() + 6.0,
        };
        ui.painter().galley(
            egui::pos2(text_x, cell.center().y - label_galley.size().y / 2.0),
            label_galley.clone(),
            color,
        );
        // 默认路径序不显示箭头（只有显式升降序才有指示，§5.1）。
        let show_chevron = sorted && !(*column == Column::Path && !sort.1);
        if show_chevron {
            // 排序指示用图标，不用 ↑↓ 字符。
            let chevron = if sort.1 {
                Icon::ChevronDown
            } else {
                Icon::ChevronUp
            };
            icons::paint_centered(
                ui.painter(),
                Rect::from_center_size(
                    egui::pos2(
                        if *column == Column::Size {
                            cell.right() - 4.0
                        } else {
                            text_x + label_galley.size().x + 8.0
                        },
                        cell.center().y,
                    ),
                    Vec2::splat(10.0),
                ),
                10.0,
                chevron,
                palette.weak,
            );
        }
        if response.clicked() {
            let (column_next, dir) = super::next_sort(sort, column.sort_column());
            let dir = dir.map_or(devtools_search_core::SortDir::None, |descending| {
                if descending {
                    devtools_search_core::SortDir::Desc
                } else {
                    devtools_search_core::SortDir::Asc
                }
            });
            action.sort = Some((column_next, dir));
        }
        // 列宽拖动。
        if index + 1 < Column::ALL.len() {
            let handle = Rect::from_center_size(
                egui::pos2(cell.right(), cell.center().y),
                Vec2::new(6.0, head_rect.height()),
            );
            let drag = ui.interact(
                handle,
                ui.id().with(("search-col-resize", index)),
                Sense::drag(),
            );
            if drag.hovered() || drag.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if drag.dragged() {
                resize_pair(&mut view.columns, index, drag.drag_delta().x);
            }
            if drag.drag_stopped() {
                action.column_resize = Some(view.columns.clone());
            }
        }
        x += width;
    }
    ui.painter().hline(
        head_rect.shrink2(Vec2::new(8.0, 0.0)).x_range(),
        head_rect.bottom() - 0.5,
        Stroke::new(1.0, palette.border.gamma_multiply(0.55)),
    );

    if empty {
        if empty_state(ui, view, service, &palette) {
            action.start_indexing = true;
        }
        return;
    }

    keyboard(ui, view, service, rows, action);

    // 结果区：行高 30、每行是一张圆角卡片。
    let row_height = theme::ROW_HEIGHT;
    // 卡片之间留 4px，和剪贴板列表一致（show_rows 会把它算进行距）。
    ui.spacing_mut().item_spacing.y = theme::ROW_GAP;
    let max_height = (ui.available_height() - theme::STATUS_HEIGHT - theme::BOTTOM_PAD - 2.0)
        .max(row_height);
    let mut area = egui::ScrollArea::vertical()
        .id_salt("search-detail")
        .auto_shrink([false, false])
        .max_height(max_height);
    if let Some(offset) = view.pending_scroll.take() {
        area = area.vertical_scroll_offset(offset);
    }
    area.show_rows(ui, row_height, rows.max(1), |ui, range| {
        view.visible = (range.start, range.end);
        // 空白层要先注册，卡片后注册才能接住自己那块区域的点击。
        selection_background(ui, view);
        view.hit_cells.clear();
        for index in range {
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), row_height),
                Sense::click(),
            );
            let data = view.cache.get(index as u32).cloned();
            // 选中范围（Shift 连选 / ⌘A 全选）。
            let selected = view.is_selected(index);
            // 每一行都是一张卡片（与剪贴板列表同一语言）：白底 + 细描边，
            // 选中时换成强调色描边，悬停时底色提亮。
            let card = Rect::from_min_max(
                egui::pos2(rect.left() + 6.0, rect.top()),
                egui::pos2(rect.right() - 6.0, rect.bottom()),
            );
            // 行数据还没加载时留空，不画只有边框的空卡片（搜索过程中会闪一下）。
            if data.is_some() {
                let (fill, stroke) = card_style(&palette, selected, response.hovered());
                ui.painter()
                    .add(theme::continuous_rect(card, 10.0, fill, stroke));
                // 框选命中测试用（下一帧生效，拖选时差一帧看不出来）。
                view.hit_cells.push((index, card));
            }
            match data {
                Some(row) if !row.missing => {
                    // 原生图标：按类型缓存，取不到就画矢量图标。
                    let icon = view.icons.icon(
                        ui.ctx(),
                        host.as_ref(),
                        row.kind,
                        &row.name,
                        std::path::Path::new(&row.path),
                    );
                    paint_row(ui, rect, &row, &widths, &palette, view, service, icon);
                    respond(ui, view, index, &response, &row, &mut *action);
                }
                Some(row) => {
                    let _ = row;
                    paint_missing(ui, rect, &palette);
                }
                None => {}
            }
        }
        paint_selection_band(ui, view, &palette);
    });
    action.requests = view.cache.take_requests(view.visible, rows);
}

/// 图标视图：按列铺卡片、按行虚拟化（`show_rows` 一次给一整行的可见行）。
fn grid_view(
    ui: &mut Ui,
    view: &mut ViewState,
    service: &Service,
    host: &std::sync::Arc<dyn crate::HostServices>,
    mode: ResultView,
    action: &mut ResultsAction,
) {
    let palette = theme::palette(ui);
    let style = GridStyle::for_view(mode);
    let rows = service.results_len();
    let empty = rows == 0 && view.total == 0;

    if empty {
        if empty_state(ui, view, service, &palette) {
            action.start_indexing = true;
        }
        return;
    }

    // 列数按可用宽度算，键盘导航与滚动换算都用它。
    // n 个卡片占 n * cell + (n-1) * gap，反过来求能放几列（别漏掉最后一个间距）；
    // 左右各留 `GRID_SIDE_PAD`，第一列和最后一列不贴边。
    let available = ui.available_width().max(style.cell_w + GRID_GAP);
    let inner = (available - GRID_SIDE_PAD * 2.0).max(style.cell_w);
    let cols = (((inner + GRID_GAP) / (style.cell_w + GRID_GAP)).floor() as usize).max(1);
    view.grid_cols = cols;
    // 列数取整之后剩下的宽度全部摊进两两之间的间距：卡片铺满整行，
    // 不在右边留一条空白（图标模式的常见排法）。
    let gap = if cols > 1 {
        ((inner - cols as f32 * style.cell_w) / (cols - 1) as f32).max(GRID_GAP)
    } else {
        GRID_GAP
    };

    keyboard(ui, view, service, rows, action);

    let row_count = rows.div_ceil(cols);
    let max_height = (ui.available_height() - theme::STATUS_HEIGHT - theme::BOTTOM_PAD - 2.0)
        .max(style.cell_h);
    ui.spacing_mut().item_spacing.y = GRID_GAP;
    let mut area = egui::ScrollArea::vertical()
        // 与详情视图分开记滚动位置：来回切换视图不会互相打断。
        .id_salt("search-grid")
        .auto_shrink([false, false])
        .max_height(max_height);
    if let Some(offset) = view.pending_scroll.take() {
        area = area.vertical_scroll_offset(offset);
    }
    area.show_rows(ui, style.cell_h, row_count, |ui, range| {
        // 可见范围换算回结果下标（加载行数据按批请求）。
        view.visible = (
            (range.start * cols).min(rows),
            (range.end * cols).min(rows),
        );
        selection_background(ui, view);
        view.hit_cells.clear();
        for row_index in range {
            let (line, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), style.cell_h),
                Sense::hover(),
            );
            for col in 0..cols {
                let index = row_index * cols + col;
                if index >= rows {
                    break;
                }
                let left = line.left() + GRID_SIDE_PAD + col as f32 * (style.cell_w + gap);
                let cell = Rect::from_min_size(
                    egui::pos2(left, line.top()),
                    Vec2::new(style.cell_w, style.cell_h),
                );
                view.hit_cells.push((index, cell));
                grid_cell(ui, view, host, cell, index, style, &palette, &mut *action);
            }
        }
        paint_selection_band(ui, view, &palette);
    });
    action.requests = view.cache.take_requests(view.visible, rows);
}

/// 画一个图标卡片（图标 + 两行文件名），并处理选中 / 打开 / 右键。
#[allow(clippy::too_many_arguments)]
fn grid_cell(
    ui: &mut Ui,
    view: &mut ViewState,
    host: &std::sync::Arc<dyn crate::HostServices>,
    cell: Rect,
    index: usize,
    style: GridStyle,
    palette: &Palette,
    action: &mut ResultsAction,
) {
    let response = ui.interact(cell, ui.id().with(("search-grid-cell", index)), Sense::click());
    let data = view.cache.get(index as u32).cloned();
    let selected = view.is_selected(index);

    // 卡片底：选中时强调色描边，悬停提亮；没拿到数据时不画（避免搜索中闪空框）。
    if data.is_some() {
        let (fill, stroke) = card_style(palette, selected, response.hovered());
        ui.painter()
            .add(theme::continuous_rect(cell, 10.0, fill, stroke));
    }

    match data {
        Some(row) if !row.missing => {
            let icon_rect = Rect::from_center_size(
                egui::pos2(cell.center().x, cell.top() + GRID_ICON_TOP + style.icon / 2.0),
                Vec2::splat(style.icon),
            );
            // 缩略图/系统图标都从后台要（目录要的是系统文件夹图标，文件先试内容预览）。
            // 框选拖动中只读缓存、不发起新请求：解码线程抢 CPU 会让拖动手感变飘。
            let size = request_size(ui.ctx(), style.icon);
            let is_dir = row.kind == NodeKind::Dir;
            let thumbnail = if view.drag_origin.is_some() {
                view.thumbs.cached(&row.path, size, is_dir)
            } else {
                view.thumbs.texture(ui.ctx(), host, &row.path, size, is_dir)
            };
            match thumbnail {
                Some(texture) => paint_thumbnail(ui.painter(), icon_rect, &texture),
                // 还没解好：先用矢量图标占位。矢量在任何尺寸下都清楚，
                // 不会出现"先糊一下、几十毫秒后变清晰"的跳变（32px 的系统小图标就会）。
                None => icons::paint_centered(
                    ui.painter(),
                    icon_rect,
                    style.icon * 0.62,
                    match row.kind {
                        NodeKind::Dir => Icon::Folder,
                        NodeKind::File => Icon::Document,
                    },
                    match row.kind {
                        NodeKind::Dir => palette.accent,
                        NodeKind::File => palette.weak,
                    },
                ),
            }
            // 文件名：卡片底部两行，居中，超长按字符截断。
            let name_rect = Rect::from_min_max(
                egui::pos2(cell.left() + 4.0, cell.bottom() - GRID_NAME_HEIGHT),
                egui::pos2(cell.right() - 4.0, cell.bottom() - 2.0),
            );
            paint_grid_name(ui.painter(), name_rect, &row.name, palette.text);
            respond(ui, view, index, &response, &row, &mut *action);
        }
        Some(_) => paint_missing(ui, cell, palette),
        None => {}
    }

}

/// 缩略图请求的像素尺寸：按当前 DPI 放大到 2× 以内，避免 Retina 下发虚。
fn request_size(ctx: &egui::Context, logical: f32) -> u32 {
    let scale = ctx.pixels_per_point().clamp(1.0, 2.0);
    (logical * scale).round().clamp(16.0, 512.0) as u32
}

/// 画缩略图：等比缩放放进 `rect`（不裁剪、不变形），四周留白。
fn paint_thumbnail(painter: &egui::Painter, rect: Rect, texture: &egui::TextureHandle) {
    let size = texture.size_vec2();
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    let draw = Rect::from_center_size(rect.center(), size * scale);
    painter.image(
        texture.id(),
        draw,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

/// 结果区的空白层：点空白取消选择，按住左键拖出一个框选（框到的项都选中）。
///
/// 必须在画卡片**之前**调用——交互是后注册的在上层，卡片才接得住自己那块区域的点击。
fn selection_background(ui: &mut Ui, view: &mut ViewState) {
    let rect = ui.available_rect_before_wrap();
    let bg = ui.interact(rect, ui.id().with("search-selection"), Sense::click_and_drag());
    if bg.clicked() {
        // 点空白：取消选中（和资源管理器一致）。
        view.clear_selection();
    }
    if bg.drag_started() {
        // 按住 Ctrl / Shift 拖动是"追加框选"，否则从空白处开始重新选。
        let keep = ui.input(|input| input.modifiers.command || input.modifiers.shift);
        view.drag_base = if keep {
            view.selected.clone()
        } else {
            Default::default()
        };
        view.drag_origin = bg.interact_pointer_pos();
    }
    if bg.drag_stopped() {
        view.drag_origin = None;
        view.drag_base.clear();
    }
    if let Some(origin) = view.drag_origin {
        if let Some(pointer) = ui.ctx().pointer_latest_pos() {
            let band = Rect::from_two_pos(origin, pointer);
            // 命中的是上一帧的几何（本帧卡片还没画），拖选时差一帧基本看不出来。
            let hits: Vec<usize> = view
                .hit_cells
                .iter()
                .filter(|(_, cell)| band.intersects(*cell))
                .map(|(index, _)| *index)
                .collect();
            let mut next = view.drag_base.clone();
            next.extend(hits);
            view.selected = next;
        }
    }
}

/// 框选时的虚线范围（画在卡片之上）。
fn paint_selection_band(ui: &Ui, view: &ViewState, palette: &Palette) {
    let Some(origin) = view.drag_origin else {
        return;
    };
    let Some(pointer) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    let band = Rect::from_two_pos(origin, pointer);
    ui.painter()
        .rect_filled(band, 2.0, palette.accent.gamma_multiply(0.12));
    ui.painter().rect_stroke(
        band,
        2.0,
        Stroke::new(1.0, palette.accent),
        egui::StrokeKind::Inside,
    );
}

/// 卡片/行在三种状态下的底色与描边。
fn card_style(palette: &Palette, selected: bool, hovered: bool) -> (Color32, Stroke) {
    if selected {
        (
            palette.card,
            Stroke::new(1.5, palette.accent.gamma_multiply(0.85)),
        )
    } else if hovered {
        (palette.card_hover, Stroke::new(1.0, palette.card_stroke))
    } else {
        (palette.card, Stroke::new(1.0, palette.card_stroke))
    }
}

/// 单击选中（Shift 连选）、双击打开、右键菜单（两种视图共用）。
fn respond(
    ui: &Ui,
    view: &mut ViewState,
    index: usize,
    response: &egui::Response,
    row: &RowData,
    action: &mut ResultsAction,
) {
    if response.clicked() {
        // Ctrl / ⌘ 切换单项（不连续多选），Shift 连选一小段，其它情况只选这一项。
        let (shift, command) = ui.input(|input| (input.modifiers.shift, input.modifiers.command));
        if command {
            view.toggle_selection(index);
        } else if shift {
            view.extend_selection_to(index);
        } else {
            view.select_only(index);
        }
    }
    if response.double_clicked() {
        view.select_only(index);
        action.open = Some(PathBuf::from(&row.path));
    }
    // 右键点未选中的项：先把它选中（和资源管理器一致），再弹菜单。
    if response.secondary_clicked() && !view.is_selected(index) {
        view.select_only(index);
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if view.system_menus {
        // Windows：交给 shell 的右键菜单（与资源管理器同一套，第三方扩展也在）。
        if response.secondary_clicked() {
            action.system_menu = Some(PathBuf::from(&row.path));
        }
        return;
    }
    response.context_menu(|ui| {
        if ui.button(format!("{}  打开", "↵")).clicked() {
            action.open = Some(PathBuf::from(&row.path));
            ui.close();
        }
        if ui.button("在文件管理器中显示").clicked() {
            action.reveal = Some(PathBuf::from(&row.path));
            ui.close();
        }
        ui.separator();
        let selected = view.selected_count();
        if selected > 1 {
            if ui.button(format!("复制这 {selected} 项的路径")).clicked() {
                action.copy = Some(join_paths(view));
                ui.close();
            }
        } else if ui.button("复制路径").clicked() {
            action.copy = Some(row.path.clone());
            ui.close();
        }
        if ui.button("复制文件名").clicked() {
            action.copy = Some(row.name.clone());
            ui.close();
        }
    });
}

/// 已被移除的行：只留一句灰色说明。
fn paint_missing(ui: &mut Ui, rect: Rect, palette: &Palette) {
    ui.painter().text(
        egui::pos2(rect.center().x, rect.center().y),
        Align2::CENTER_CENTER,
        "已被移除",
        FontId::proportional(theme::TEXT_HINT),
        palette.faint,
    );
}

/// 画图标视图的文件名：居中、最多两行、超长用 `…`。
fn paint_grid_name(painter: &egui::Painter, rect: Rect, text: &str, color: Color32) {
    let font = FontId::proportional(12.0);
    let mut job = LayoutJob::default();
    // 设置了 `halign` 之后，每一行都以绘制原点的 x 为中线：原点给卡片中线，
    // 而不是左边（否则整段文字会向左溢出半行宽，文件名只剩后半截）。
    job.halign = egui::Align::Center;
    job.wrap = TextWrapping {
        max_width: rect.width(),
        max_rows: 2,
        break_anywhere: true,
        overflow_character: Some('…'),
        ..Default::default()
    };
    job.append(text, 0.0, TextFormat { font_id: font, color, ..Default::default() });
    let galley = painter.layout_job(job);
    painter.with_clip_rect(rect).galley(
        egui::pos2(rect.center().x, rect.top()),
        galley,
        color,
    );
}

/// 缩放相邻两列的权重。
fn resize_pair(columns: &mut ColumnWidths, index: usize, delta: f32) {
    let (left, right) = match index {
        0 => (0, 1),
        1 => (1, 2),
        2 => (2, 3),
        3 => (3, 4),
        _ => return,
    };
    let mut values = [
        columns.name,
        columns.path,
        columns.size,
        columns.modified,
        columns.created,
    ];
    if values[left] + delta < 60.0 || values[right] - delta < 50.0 {
        return;
    }
    values[left] += delta;
    values[right] -= delta;
    columns.name = values[0];
    columns.path = values[1];
    columns.size = values[2];
    columns.modified = values[3];
    columns.created = values[4];
}

/// 按权重把可用宽度分给五列。
fn column_widths(view: &ViewState, available: f32) -> [f32; 5] {
    let weights = [
        view.columns.name,
        view.columns.path,
        view.columns.size,
        view.columns.modified,
        view.columns.created,
    ];
    let total: f32 = weights.iter().sum();
    let usable = (available - 12.0).max(200.0);
    weights.map(|w| (w / total * usable).max(24.0))
}

/// 画一行：文件图标 + 名称（可高亮）+ 所在目录 + 大小 + 时间。
fn paint_row(
    ui: &Ui,
    rect: Rect,
    row: &crate::application::protocol::RowData,
    widths: &[f32; 5],
    palette: &Palette,
    view: &ViewState,
    service: &Service,
    icon: Option<egui::TextureHandle>,
) {
    let painter = ui.painter();
    let mut x = rect.left() + 8.0;
    // 文件名：系统图标（取不到时回退矢量图标）+ 文本。
    let icon_rect = Rect::from_center_size(egui::pos2(x + 16.0, rect.center().y), Vec2::splat(18.0));
    match icon {
        Some(texture) => {
            painter.image(
                texture.id(),
                icon_rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => icons::paint_centered(
            painter,
            icon_rect,
            15.0,
            match row.kind {
                NodeKind::Dir => Icon::Folder,
                NodeKind::File => Icon::Document,
            },
            match row.kind {
                NodeKind::Dir => palette.accent,
                NodeKind::File => palette.weak,
            },
        ),
    }
    let name_rect = Rect::from_min_size(
        egui::pos2(x + 30.0, rect.top()),
        Vec2::new((widths[0] - 30.0).max(40.0), rect.height()),
    );
    let needle = if service.settings().highlight {
        let q = view.submitted.trim();
        (!q.is_empty() && !q.contains(' ') && !q.contains(':') && !q.contains('|') && !q.contains('/'))
            .then_some(q)
    } else {
        None
    };
    paint_name(
        painter,
        name_rect,
        &row.name,
        theme::TEXT_BODY,
        palette.text,
        needle,
        view.case_sensitive,
        palette.accent.gamma_multiply(0.25),
    );
    x += widths[0];
    // 所在目录：中间省略。
    let path_cell = Rect::from_min_size(
        egui::pos2(x, rect.top()),
        Vec2::new(widths[1], rect.height()),
    );
    let path_text = theme::middle_ellipsis(
        ui,
        &row.parent,
        FontId::proportional(theme::TEXT_HINT),
        widths[1] - 14.0,
    );
    // 列宽被拖到很窄时文本仍可能超出一两个像素，按格子裁剪，绝不画到右边的列上。
    painter.with_clip_rect(path_cell).text(
        egui::pos2(path_cell.left() + 6.0, path_cell.center().y),
        Align2::LEFT_CENTER,
        path_text,
        FontId::proportional(theme::TEXT_HINT),
        palette.weak,
    );
    x += widths[1];
    // 大小（右对齐、等宽数字）。
    let size_cell = Rect::from_min_size(
        egui::pos2(x, rect.top()),
        Vec2::new(widths[2], rect.height()),
    );
    painter.with_clip_rect(size_cell).text(
        egui::pos2(size_cell.right() - 10.0, size_cell.center().y),
        Align2::RIGHT_CENTER,
        match (row.kind, row.size) {
            (NodeKind::File, Some(size)) => theme::byte_size(size),
            _ => "—".to_string(),
        },
        FontId::monospace(theme::TEXT_HINT),
        palette.weak,
    );
    x += widths[2];
    // 修改时间。
    let mtime_cell = Rect::from_min_size(
        egui::pos2(x, rect.top()),
        Vec2::new(widths[3], rect.height()),
    );
    painter.with_clip_rect(mtime_cell).text(
        egui::pos2(mtime_cell.left() + 6.0, mtime_cell.center().y),
        Align2::LEFT_CENTER,
        theme::time_label(row.mtime),
        FontId::proportional(theme::TEXT_HINT),
        palette.weak,
    );
    x += widths[3];
    // 创建时间。
    let btime_cell = Rect::from_min_size(
        egui::pos2(x, rect.top()),
        Vec2::new(widths[4], rect.height()),
    );
    painter.with_clip_rect(btime_cell).text(
        egui::pos2(btime_cell.left() + 6.0, btime_cell.center().y),
        Align2::LEFT_CENTER,
        theme::time_label(row.btime),
        FontId::proportional(theme::TEXT_HINT),
        palette.weak,
    );
}

/// 画文件名，匹配部分用强调色底高亮（ASCII 才做下标映射，避免多字节错位）。
///
/// 文件名按列宽单行截断、结尾换成 `…`：不做换行也不截断时，长文件名会画到路径列上面。
fn paint_name(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    size: f32,
    color: Color32,
    needle: Option<&str>,
    case_sensitive: bool,
    highlight: Color32,
) {
    let font = FontId::proportional(size);
    let normal = TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let marked = TextFormat {
        font_id: font.clone(),
        color,
        background: highlight,
        ..Default::default()
    };
    let range = needle.and_then(|needle| {
        if !text.is_ascii() || !needle.is_ascii() {
            return None;
        }
        let (haystack, find) = if case_sensitive {
            (text.to_string(), needle.to_string())
        } else {
            (text.to_ascii_lowercase(), needle.to_ascii_lowercase())
        };
        let pos = haystack.find(&find)?;
        Some((pos, pos + find.len()))
    });
    let mut job = LayoutJob::default();
    job.wrap = TextWrapping {
        max_width: rect.width(),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
        ..Default::default()
    };
    match range {
        Some((start, end)) => {
            job.append(&text[..start], 0.0, normal.clone());
            job.append(&text[start..end], 0.0, marked);
            job.append(&text[end..], 0.0, normal);
        }
        None => job.append(text, 0.0, normal),
    }
    let galley = painter.layout_job(job);
    let y = rect.center().y - galley.size().y / 2.0;
    // 截断已经保证宽度不超，再按格子裁剪一层，避免字距等细节在边界上越界。
    painter
        .with_clip_rect(rect)
        .galley(egui::pos2(rect.left(), y), galley, color);
}

/// 键盘：↑↓（图标视图里映射到整行）、←→（图标视图）、PgUp/PgDn、回车打开、
/// ⌘O 打开、⌘R 显示、⇧⌘C 复制路径、Esc 清空/收起。
fn keyboard(
    ui: &mut Ui,
    view: &mut ViewState,
    service: &Service,
    rows: usize,
    action: &mut ResultsAction,
) {
    if rows == 0 {
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if view.query.is_empty() {
                action.close = true;
            } else {
                view.query.clear();
                view.last_edit = None;
            }
        }
        return;
    }
    let rows_count = rows;
    let mode = service.settings().ui.view;
    let grid = mode.is_grid();
    // 图标视图一次移动一整行（列数来自上一帧的布局）。
    let cols = view.grid_cols.max(1);
    let step = if grid { cols } else { 1 };
    let page = if grid {
        // 一屏能看到多少项（`visible` 是结果下标区间）。
        (view.visible.1.saturating_sub(view.visible.0)).max(cols)
    } else {
        (view.visible.1.saturating_sub(view.visible.0)).max(1)
    };
    let mut moved = false;
    ui.input_mut(|input| {
        // 带 Shift 时把移动经过的一段并入选择（锚点不动），否则只选目标项。
        let extend = |input: &mut egui::InputState, view: &mut ViewState, target: usize| {
            if input.modifiers.shift {
                view.extend_selection_to(target);
            } else {
                view.select_only(target);
            }
        };
        if input.consume_key(Modifiers::NONE, Key::ArrowDown) || input.consume_key(Modifiers::SHIFT, Key::ArrowDown) {
            let target = (view.cursor + step).min(rows_count - 1);
            extend(input, view, target);
            moved = true;
        }
        if input.consume_key(Modifiers::NONE, Key::ArrowUp) || input.consume_key(Modifiers::SHIFT, Key::ArrowUp) {
            let target = view.cursor.saturating_sub(step);
            extend(input, view, target);
            moved = true;
        }
        // 图标视图里左右键按项移动；详情视图不处理，输入框有焦点时也留给输入框。
        if grid
            && !view.search_focused
            && (input.consume_key(Modifiers::NONE, Key::ArrowRight)
                || input.consume_key(Modifiers::SHIFT, Key::ArrowRight))
        {
            let target = (view.cursor + 1).min(rows_count - 1);
            extend(input, view, target);
            moved = true;
        }
        if grid
            && !view.search_focused
            && (input.consume_key(Modifiers::NONE, Key::ArrowLeft)
                || input.consume_key(Modifiers::SHIFT, Key::ArrowLeft))
        {
            let target = view.cursor.saturating_sub(1);
            extend(input, view, target);
            moved = true;
        }
        if input.consume_key(Modifiers::NONE, Key::PageDown) || input.consume_key(Modifiers::SHIFT, Key::PageDown) {
            let target = (view.cursor + page).min(rows_count - 1);
            extend(input, view, target);
            moved = true;
        }
        if input.consume_key(Modifiers::NONE, Key::PageUp) || input.consume_key(Modifiers::SHIFT, Key::PageUp) {
            let target = view.cursor.saturating_sub(page);
            extend(input, view, target);
            moved = true;
        }
        // ⌘A 全选结果；搜索框有焦点时它归输入框（全选文字），这里不能抢。
        if !view.search_focused && input.consume_key(Modifiers::COMMAND, Key::A) {
            view.select_all(rows_count);
        }
        if input.consume_key(Modifiers::NONE, Key::Escape) {
            if view.selected_count() > 1 {
                // 先收成只选当前项，再按 Esc 才清空搜索 / 收起窗口。
                let cursor = view.cursor;
                view.select_only(cursor);
            } else if view.query.is_empty() {
                action.close = true;
            } else {
                view.query.clear();
                view.last_edit = None;
            }
        }
    });
    if moved {
        // 把光标所在项滚进视野：详情按行算、图标按行号（下标 / 列数）算。
        let pitch = if grid {
            GridStyle::for_view(mode).cell_h + GRID_GAP
        } else {
            theme::ROW_HEIGHT + theme::ROW_GAP
        };
        let row_of = |index: usize| {
            if grid {
                index / cols
            } else {
                index
            }
        };
        let target_row = row_of(view.cursor);
        if view.cursor < view.visible.0 {
            view.pending_scroll = Some(target_row as f32 * pitch);
        } else if view.cursor >= view.visible.1 {
            let height = ui.available_height();
            view.pending_scroll =
                Some((target_row as f32 * pitch - height + pitch * 2.0).max(0.0));
        }
    }
    let open = ui.input_mut(|input| {
        input.consume_key(Modifiers::COMMAND, Key::O)
            || input.consume_key(Modifiers::NONE, Key::Enter)
    });
    let reveal = ui.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::R));
    let copy = ui.input_mut(|input| {
        input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::C)
    });
    if copy {
        action.copy = Some(if view.selected_count() > 1 {
            join_paths(view)
        } else {
            view.cache
                .get(view.cursor as u32)
                .map(|row| row.path.clone())
                .unwrap_or_default()
        });
    }
    if let Some(row) = view.cache.get(view.cursor as u32) {
        if !row.missing {
            if open {
                action.open = Some(PathBuf::from(&row.path));
            }
            if reveal {
                action.reveal = Some(PathBuf::from(&row.path));
            }
        }
    }
}

/// 把选中的项拼成多行路径（未加载的行跳过，避免为复制再拉一次数据）。
fn join_paths(view: &ViewState) -> String {
    view.selected
        .iter()
        .filter_map(|&index| view.cache.get(index as u32))
        .filter(|row| !row.missing)
        .map(|row| row.path.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 空态：大图标 + 标题 + 说明；索引未建立时给出主按钮。返回是否点了"建立索引"。
fn empty_state(ui: &mut Ui, view: &ViewState, service: &Service, palette: &Palette) -> bool {
    let mut start_indexing = false;
    // 给底部状态栏留出位置：空态只在结果区居中，状态栏始终可见。
    let height = (ui.available_height() - theme::STATUS_HEIGHT - 2.0).max(120.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let center = rect.center() - Vec2::new(0.0, 18.0);

    // 图标圆底。
    let badge = Rect::from_center_size(center - Vec2::new(0.0, 34.0), Vec2::splat(52.0));
    ui.painter()
        .circle_filled(badge.center(), 26.0, palette.field);
    icons::paint_centered(
        ui.painter(),
        badge,
        24.0,
        Icon::Search,
        palette.weak.gamma_multiply(0.9),
    );

    let kind = service.settings().ui.kind;
    let (title, hint) = if let Some(error) = &view.error {
        ("查询有误", error.clone())
    } else if view.searching {
        ("搜索中…", "正在查找匹配的文件".to_string())
    } else if view.query.trim().is_empty() {
        match view.lifecycle {
            // `lazy_start` 下启动时不加载索引（省内存和启动时间），第一次搜索才载入：
            // 有快照就先出结果再后台校验，没快照就是首次建立。文案要同时覆盖这两种情况。
            Lifecycle::Dormant => (
                "索引尚未载入",
                "输入关键词开始搜索：会先载入上次的索引，再在后台校验变化".to_string(),
            ),
            Lifecycle::AwaitingConsent => ("等待授权", "完成首次引导后即可搜索文件".to_string()),
            _ => (
                "开始搜索",
                "输入文件名、路径片段，或用 ext:pdf、folder:、project/report 收窄范围".to_string(),
            ),
        }
    } else if let Some(notice) = &view.notice {
        ("没有结果", notice.clone())
    } else if kind != FileKind::All {
        (
            "没有匹配的文件",
            format!("当前只看「{}」，把类型切回「所有」再试试", kind.label()),
        )
    } else {
        (
            "没有匹配的文件",
            "试试更短的关键词，或检查排除规则".to_string(),
        )
    };
    ui.painter().text(
        center + Vec2::new(0.0, 0.0),
        Align2::CENTER_CENTER,
        title,
        theme::title_font(ui, theme::TEXT_TITLE),
        if view.error.is_some() {
            palette.danger
        } else {
            palette.text
        },
    );
    ui.painter().text(
        center + Vec2::new(0.0, 22.0),
        Align2::CENTER_CENTER,
        if hint.chars().count() > 46 {
            format!("{}…", hint.chars().take(44).collect::<String>())
        } else {
            hint
        },
        FontId::proportional(theme::TEXT_HINT),
        palette.weak,
    );

    // 索引没建立时给一个主按钮（lazy_start 的首次使用）。
    if view.lifecycle == Lifecycle::Dormant && !service.read_only() {
        let button_rect = Rect::from_center_size(
            center + Vec2::new(0.0, 56.0),
            Vec2::new(140.0, 26.0),
        );
        let mut button_ui = ui.new_child(egui::UiBuilder::new().max_rect(button_rect));
        if theme::primary_button(
            &mut button_ui,
            Some(Icon::Search),
            "立即建立索引",
            palette,
        )
        .clicked()
        {
            start_indexing = true;
        }
    }
    start_indexing
}
