//! 结果表：五列虚拟化表格、卡片式行、键盘操作与右键菜单（§5.1、§5.2）。

use std::path::PathBuf;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, CursorIcon, FontId, Key, Modifiers, Rect, Sense, Stroke, Ui, Vec2,
};
use devtools_search_core::{NodeKind, SortColumn};

use super::cache::CHUNK;
use super::icons::{self, Icon};
use super::theme::{self, Palette};
use super::ViewState;
use crate::application::service::{Lifecycle, Service};
use crate::settings::ColumnWidths;

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

pub fn show(
    ui: &mut Ui,
    view: &mut ViewState,
    service: &Service,
    host: &dyn crate::HostServices,
) -> ResultsAction {
    let palette = theme::palette(ui);
    let mut action = ResultsAction::default();
    ui.spacing_mut().item_spacing = Vec2::ZERO;

    let available = ui.available_width().max(360.0);
    let widths = column_widths(view, available);

    // 空态：不画表头，直接给引导（大图标 + 说明 + 主按钮）。
    let rows = service.results_len();
    if view.total == 0 && !view.searching {
        if empty_state(ui, view, service, &palette) {
            action.start_indexing = true;
        }
        return action;
    }

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

    keyboard(ui, view, service, rows, &mut action);

    // 结果区：行高 30、每行是一张圆角卡片。
    let row_height = theme::ROW_HEIGHT;
    // 卡片之间留 4px，和剪贴板列表一致（show_rows 会把它算进行距）。
    ui.spacing_mut().item_spacing.y = theme::ROW_GAP;
    let max_height = (ui.available_height() - theme::STATUS_HEIGHT - 2.0).max(row_height);
    let mut area = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(max_height);
    if let Some(offset) = view.pending_scroll.take() {
        area = area.vertical_scroll_offset(offset);
    }
    area.show_rows(ui, row_height, rows.max(1), |ui, range| {
        view.visible = (range.start, range.end);
        for index in range {
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), row_height),
                Sense::click(),
            );
            let data = view.cache.get(index as u32).cloned();
            // 选中范围（Shift 连选 / ⌘A 全选）。
            let selected = view.selection_range(rows).contains(&index);
            // 每一行都是一张卡片（与剪贴板列表同一语言）：白底 + 细描边，
            // 选中时换成强调色描边，悬停时底色提亮。
            let card = Rect::from_min_max(
                egui::pos2(rect.left() + 6.0, rect.top()),
                egui::pos2(rect.right() - 6.0, rect.bottom()),
            );
            let (fill, stroke) = if selected {
                (
                    palette.card,
                    Stroke::new(1.5, palette.accent.gamma_multiply(0.85)),
                )
            } else if response.hovered() {
                (palette.card_hover, Stroke::new(1.0, palette.card_stroke))
            } else {
                (palette.card, Stroke::new(1.0, palette.card_stroke))
            };
            ui.painter()
                .add(theme::continuous_rect(card, 10.0, fill, stroke));
            match data {
                Some(row) if !row.missing => {
                    // 原生图标：按类型缓存，取不到就画矢量图标。
                    let icon = view.icons.icon(
                        ui.ctx(),
                        host,
                        row.kind,
                        &row.name,
                        std::path::Path::new(&row.path),
                    );
                    paint_row(ui, rect, &row, &widths, &palette, view, service, icon);
                    if response.clicked() {
                        let shift = ui.input(|input| input.modifiers.shift);
                        view.cursor = index;
                        if !shift {
                            view.anchor = index;
                        }
                    }
                    if response.double_clicked() {
                        view.cursor = index;
                        action.open = Some(PathBuf::from(&row.path));
                    }
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    }
                    response.context_menu(|ui| {
                        if ui
                            .button(format!("{}  打开", "↵"))
                            .clicked()
                        {
                            action.open = Some(PathBuf::from(&row.path));
                            ui.close();
                        }
                        if ui.button("在文件管理器中显示").clicked() {
                            action.reveal = Some(PathBuf::from(&row.path));
                            ui.close();
                        }
                        ui.separator();
                        let selected = view.selected_count(rows);
                        if selected > 1 {
                            if ui
                                .button(format!("复制这 {selected} 项的路径"))
                                .clicked()
                            {
                                action.copy = Some(join_paths(view, rows));
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
                Some(_) | None => {
                    let missing = data.as_ref().is_some_and(|r| r.missing);
                    if missing {
                        ui.painter().text(
                            egui::pos2(rect.left() + 40.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            "已被移除",
                            FontId::proportional(theme::TEXT_HINT),
                            palette.faint,
                        );
                    }
                }
            }
        }
    });
    action.requests = view.cache.take_requests(view.visible, rows);
    action
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

/// 键盘：↑↓ 移动、PgUp/PgDn、回车打开、⌘O 打开、⌘R 显示、⇧⌘C 复制路径、Esc 清空/收起。
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
    let page = (view.visible.1.saturating_sub(view.visible.0)).max(1);
    let mut moved = false;
    ui.input_mut(|input| {
        // 带 Shift 时扩展选区（锚点不动），否则移动并把锚点收回当前行。
        let extend = |input: &mut egui::InputState, view: &mut ViewState, target: usize| {
            if input.modifiers.shift {
                view.cursor = target;
            } else {
                view.cursor = target;
                view.anchor = target;
            }
        };
        if input.consume_key(Modifiers::NONE, Key::ArrowDown) || input.consume_key(Modifiers::SHIFT, Key::ArrowDown) {
            let target = (view.cursor + 1).min(rows_count - 1);
            extend(input, view, target);
            moved = true;
        }
        if input.consume_key(Modifiers::NONE, Key::ArrowUp) || input.consume_key(Modifiers::SHIFT, Key::ArrowUp) {
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
        // ⌘A 全选。
        if input.consume_key(Modifiers::COMMAND, Key::A) {
            view.anchor = 0;
            view.cursor = rows_count - 1;
        }
        if input.consume_key(Modifiers::NONE, Key::Escape) {
            if view.selected_count(rows_count) > 1 {
                // 先取消多选，再按 Esc 才清空搜索 / 收起窗口。
                view.anchor = view.cursor;
            } else if view.query.is_empty() {
                action.close = true;
            } else {
                view.query.clear();
                view.last_edit = None;
            }
        }
    });
    if moved {
        let row_height = theme::ROW_HEIGHT;
        if view.cursor < view.visible.0 {
            view.pending_scroll = Some(view.cursor as f32 * row_height);
        } else if view.cursor >= view.visible.1 {
            let height = ui.available_height();
            view.pending_scroll =
                Some(((view.cursor as f32) * row_height - height + row_height * 2.0).max(0.0));
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
        action.copy = Some(if view.selected_count(rows_count) > 1 {
            join_paths(view, rows_count)
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
    let _ = service;
}

/// 把选中范围内已缓存的行路径拼成多行（未加载的行跳过，避免为复制再拉一次数据）。
fn join_paths(view: &ViewState, rows: usize) -> String {
    view.selection_range(rows)
        .filter_map(|index| view.cache.get(index as u32))
        .filter(|row| !row.missing)
        .map(|row| row.path.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 空态：大图标 + 标题 + 说明；索引未建立时给出主按钮。返回是否点了"建立索引"。
fn empty_state(ui: &mut Ui, view: &ViewState, service: &Service, palette: &Palette) -> bool {
    let mut start_indexing = false;
    let height = ui.available_height().max(120.0);
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

    let (title, hint) = if let Some(error) = &view.error {
        ("查询有误", error.clone())
    } else if view.query.trim().is_empty() {
        match view.lifecycle {
            Lifecycle::Dormant => (
                "索引尚未建立",
                "输入关键词开始搜索，首次搜索会在后台建立索引".to_string(),
            ),
            Lifecycle::AwaitingConsent => ("等待授权", "完成首次引导后即可搜索文件".to_string()),
            _ => (
                "开始搜索",
                "输入文件名、路径片段，或用 ext:pdf、folder:、project/report 收窄范围".to_string(),
            ),
        }
    } else if let Some(notice) = &view.notice {
        ("没有结果", notice.clone())
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
    let _ = service;
    let _ = CHUNK;
    start_indexing
}
