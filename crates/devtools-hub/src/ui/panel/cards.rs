//! 卡片列表：只绘制可见范围内的卡片，每张卡片显示来源应用图标、内容预览、时间与快捷键。

use devtools_clipboard::store::now_ms;
use devtools_clipboard::{ClipItem, ContentKind, FAVORITES_LIST_ID};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui, Vec2};

use super::{Action, Filter};
use crate::keys;
use crate::ui::code_text;
use crate::ui::fonts;
use crate::ui::glass;
use crate::ui::icons::{self, Icon};
use crate::ui::style::{self, kind_color, kind_icon, Palette};
use crate::ui::{HubApp, PasteMode};

/// 卡片高度与卡片之间的间距（固定高度，滚动时可以直接算出可见范围）。
const CARD_HEIGHT: f32 = 74.0;
const CARD_GAP: f32 = 8.0;
/// 收藏星标的颜色。
const FAVORITE_COLOR: Color32 = Color32::from_rgb(0xf5, 0xa6, 0x23);
/// 卡片左右离窗口边缘的距离。
const SIDE: f32 = style::PAD as f32;

impl HubApp {
    /// 卡片列表。`top`、`bottom` 是浮在上面、下面的两个栏的高度：列表占满整个窗口，
    /// 内容上下留出这两段空白；滚到栏下面的卡片不直接画（裁剪掉），露出栏的玻璃背景。
    /// 返回卡片画在图层里的哪一段，没有原生玻璃时栏把它们读回去画成模糊的样子。
    pub(super) fn card_list(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        actions: &mut Vec<Action>,
        top: f32,
        bottom: f32,
    ) -> Option<glass::Source> {
        if self.panel.items.is_empty() {
            ui.add_space(top + (ui.available_height() - top - bottom) * 0.28);
            ui.vertical_centered(|ui| {
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
                icons::paint(ui.painter(), icon_rect, Icon::Document, palette.weak);
                ui.add_space(6.0);
                let text = if self.panel.query.is_empty() && self.panel.filter == Filter::All {
                    "还没有记录\n复制任意内容后会出现在这里"
                } else {
                    "没有匹配的条目"
                };
                ui.label(RichText::new(text).color(palette.weak));
            });
            return None;
        }

        let stride = CARD_HEIGHT + CARD_GAP;
        let area = ui.max_rect();
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("items")
            .auto_shrink([false, false])
            .scroll_bar_rect(Rect::from_min_max(
                area.min + Vec2::new(0.0, top),
                area.max - Vec2::new(0.0, bottom),
            ));
        if std::mem::take(&mut self.panel.scroll_to_selected) {
            // 卡片在内容里的位置；可见范围是视口去掉顶部栏挡住的部分。
            let card_top = top + self.panel.selected as f32 * stride;
            let card_bottom = card_top + CARD_HEIGHT;
            let view = (self.panel.viewport_height - bottom).max(top + CARD_HEIGHT);
            if card_top - top < self.panel.scroll_offset {
                scroll = scroll.vertical_scroll_offset(card_top - top);
            } else if card_bottom > self.panel.scroll_offset + view {
                scroll = scroll.vertical_scroll_offset(card_bottom - view + CARD_GAP);
            }
        }
        let count = self.panel.items.len();
        let mut list = None;
        let output = scroll.show_viewport(ui, |ui, viewport| {
            ui.set_min_height(top + count as f32 * stride + bottom);
            let clip = ui.clip_rect().intersect(Rect::from_min_max(
                egui::pos2(f32::NEG_INFINITY, area.top() + top),
                egui::pos2(f32::INFINITY, area.bottom() - bottom),
            ));
            ui.set_clip_rect(clip);
            let layer = ui.layer_id();
            let start = glass::Source::shape_count(ui.ctx(), layer);
            let origin = ui.max_rect().min;
            let first = ((viewport.top() - top) / stride).floor().max(0.0) as usize;
            let last = (((viewport.bottom() - top) / stride).ceil().max(0.0) as usize).min(count);
            for index in first..last {
                let item = self.panel.items[index].clone();
                let rect = Rect::from_min_size(
                    origin + Vec2::new(0.0, top + index as f32 * stride),
                    Vec2::new(ui.max_rect().width(), CARD_HEIGHT),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    self.card(ui, index, &item, palette, actions)
                });
            }
            let end = glass::Source::shape_count(ui.ctx(), layer);
            list = Some(glass::Source {
                layer,
                shapes: start..end,
            });
        });
        self.panel.scroll_offset = output.state.offset.y;
        self.panel.viewport_height = output.inner_rect.height();
        list
    }

    /// 一张卡片：单击选中，双击粘贴，右键弹出菜单。
    fn card(
        &mut self,
        ui: &mut Ui,
        index: usize,
        item: &ClipItem,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) {
        let (row, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), CARD_HEIGHT), Sense::click());
        // 左右留白放在卡片内部，浮动滚动条落在右侧空白里。
        let rect = row.shrink2(Vec2::new(SIDE, 0.0));
        if response.clicked() || response.secondary_clicked() {
            self.panel.selected = index;
        }
        if response.secondary_clicked() {
            actions.push(Action::ItemMenu(item.id));
        }
        if response.double_clicked() {
            actions.push(Action::Paste(item.id, PasteMode::Original));
        }
        let selected = index == self.panel.selected;
        let painter = ui.painter_at(row);
        let fill = if response.hovered() {
            palette.card_hover
        } else {
            palette.card
        };
        let radius = f32::from(style::CARD_RADIUS);
        painter.add(style::continuous_rect(
            rect,
            radius,
            fill,
            Stroke::new(1.0, palette.card_stroke),
        ));
        if selected {
            painter.add(style::continuous_rect(
                rect,
                radius,
                Color32::TRANSPARENT,
                Stroke::new(2.0, palette.accent),
            ));
        }

        // 左侧：来源应用图标；没有图标时显示类型徽标。
        let icon_rect = Rect::from_center_size(
            egui::pos2(rect.left() + 28.0, rect.center().y),
            Vec2::splat(30.0),
        );
        let icon = item
            .source
            .bundle_id
            .as_deref()
            .and_then(|bundle| self.app_icons.get(ui.ctx(), bundle));
        match icon {
            Some(texture) => egui::Image::from_texture(&texture).paint_at(ui, icon_rect),
            None => {
                let color = kind_color(item.kind);
                painter.rect_filled(icon_rect, style::SMALL_RADIUS, color.gamma_multiply(0.16));
                icons::paint_centered(&painter, icon_rect, 16.0, kind_icon(item.kind), color);
            }
        }

        // 右侧：时间、收藏标记、快捷键。
        let right = rect.right() - 12.0;
        let time = painter.text(
            egui::pos2(right, rect.top() + 10.0),
            Align2::RIGHT_TOP,
            style::card_time(item.last_used_at_ms, now_ms()),
            FontId::proportional(11.0),
            palette.weak,
        );
        if item.in_list(FAVORITES_LIST_ID) {
            let star = Rect::from_center_size(
                egui::pos2(time.left() - 9.0, time.center().y),
                Vec2::splat(11.0),
            );
            icons::paint(&painter, star, Icon::StarFilled, FAVORITE_COLOR);
        }
        if index < 9 {
            painter.text(
                egui::pos2(right, rect.bottom() - 10.0),
                Align2::RIGHT_BOTTOM,
                keys::shortcut(&[keys::COMMAND, &(index + 1).to_string()]),
                FontId::proportional(11.5),
                palette.weak,
            );
        }

        // 中间：内容预览。
        let content = Rect::from_min_max(
            egui::pos2(icon_rect.right() + 12.0, rect.top() + 10.0),
            egui::pos2(right - 44.0, rect.bottom() - 10.0),
        );
        match item.kind {
            ContentKind::Image => {
                let texture = item
                    .has_thumbnail
                    .then(|| self.textures.get(ui.ctx(), &self.store, item.id))
                    .flatten();
                if let Some(texture) = texture {
                    let size = fit(texture.size_vec2(), content.size());
                    let image_rect = Rect::from_min_size(
                        egui::pos2(content.left(), content.center().y - size.y / 2.0),
                        size,
                    );
                    egui::Image::from_texture(&texture)
                        .corner_radius(style::SMALL_RADIUS)
                        .paint_at(ui, image_rect);
                } else {
                    let text = item
                        .image_size
                        .map_or_else(|| "图片".to_string(), |(w, h)| format!("图片 {w} × {h}"));
                    let galley = wrapped(ui, &text, 13.5, palette.text, content.width(), 1);
                    let pos =
                        egui::pos2(content.left(), content.center().y - galley.size().y / 2.0);
                    painter.galley(pos, galley, palette.text);
                }
            }
            ContentKind::Files => {
                let text_left = content.left() + 20.0;
                let width = content.right() - text_left;
                // 与 PasteNow 一样显示完整路径：单个文件最多两行；多个文件显示第一个，下面注明数量。
                let multiple = item.file_count > 1;
                let title = match &item.first_path {
                    Some(path) => {
                        path_galley(ui, path, palette, width, if multiple { 1 } else { 2 })
                    }
                    None => wrapped(ui, &item.title, 13.5, palette.text, width, 1),
                };
                let subtitle = multiple.then(|| {
                    let text = format!("{} 个文件", item.file_count);
                    wrapped(ui, &text, 11.5, palette.weak, width, 1)
                });
                let title_height = title.size().y;
                let row_height = title_height / title.rows.len().max(1) as f32;
                let total = title_height + subtitle.as_ref().map_or(0.0, |s| 4.0 + s.size().y);
                let top = content.center().y - total / 2.0;
                let icon = Rect::from_center_size(
                    egui::pos2(content.left() + 7.0, top + row_height / 2.0),
                    Vec2::splat(14.0),
                );
                icons::paint(&painter, icon, Icon::Document, palette.weak);
                painter.galley(egui::pos2(text_left, top), title, palette.text);
                if let Some(subtitle) = subtitle {
                    painter.galley(
                        egui::pos2(text_left, top + title_height + 4.0),
                        subtitle,
                        palette.weak,
                    );
                }
            }
            kind => {
                let color = if kind == ContentKind::Link {
                    palette.accent
                } else {
                    palette.text
                };
                let text = if item.title.is_empty() {
                    kind.label()
                } else {
                    item.title.as_str()
                };
                // 从 IDE 复制的代码按富文本里的高亮显示。
                let galley = match self.code.get(&self.store, item) {
                    Some(runs) => ui.painter().layout_job(code_text::card_job(
                        &runs,
                        palette,
                        12.5,
                        content.width(),
                        2,
                    )),
                    None => wrapped(ui, text, 13.5, color, content.width(), 2),
                };
                let pos = egui::pos2(content.left(), content.center().y - galley.size().y / 2.0);
                painter.galley(pos, galley, color);
            }
        }
    }
}

/// 按宽度折行，最多 `max_rows` 行，超出部分用省略号。
fn wrapped(
    ui: &Ui,
    text: &str,
    size: f32,
    color: Color32,
    width: f32,
    max_rows: usize,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple(text.to_owned(), FontId::proportional(size), color, width);
    job.wrap = TextWrapping {
        max_width: width,
        max_rows,
        // 单行时允许任意位置截断；多行时优先在单词边界换行（中文本身可在字间换行）。
        break_anywhere: max_rows == 1,
        ..Default::default()
    };
    ui.painter().layout_job(job)
}

/// 文件路径：目录用弱色，文件名用正文色加粗；最多 `max_rows` 行。
///
/// 一行放得下就放一行；放不下时在最后一个分隔符处换行，目录占第一行、文件名占第二行，
/// 目录太长就从左边逐级省略（`…/download/`），尽量不把文件名拆开。
fn path_galley(
    ui: &Ui,
    path: &str,
    palette: &Palette,
    width: f32,
    max_rows: usize,
) -> std::sync::Arc<egui::Galley> {
    const SEPARATORS: [char; 2] = ['/', '\\'];
    let split = path
        .trim_end_matches(SEPARATORS)
        .rfind(SEPARATORS)
        .map_or(0, |index| index + 1);
    let (dir, name) = path.split_at(split);
    let separator = dir.chars().last().unwrap_or('/');
    let segments: Vec<&str> = dir.split_inclusive(SEPARATORS).collect();
    let dir_len = dir.chars().count();
    // 候选目录：完整目录，然后从左边逐级省略（只保留比原目录短的写法）。
    let mut dirs = vec![dir.to_string()];
    dirs.extend(
        (1..=segments.len())
            .map(|skip| format!("…{separator}{}", segments[skip..].concat()))
            .filter(|shown| shown.chars().count() < dir_len),
    );
    let layout = |dir: &str, newline: bool| {
        let dir_format = TextFormat::simple(FontId::proportional(13.5), palette.weak);
        let mut job = LayoutJob::default();
        job.append(dir, 0.0, dir_format.clone());
        if newline && !dir.is_empty() {
            job.append("\n", 0.0, dir_format);
        }
        job.append(
            name,
            0.0,
            TextFormat::simple(
                FontId::new(13.5, egui::FontFamily::Name(fonts::SEMIBOLD.into())),
                palette.text,
            ),
        );
        job.wrap = TextWrapping {
            max_width: width,
            max_rows,
            break_anywhere: true,
            ..Default::default()
        };
        ui.painter().layout_job(job)
    };
    let newline = max_rows > 1;
    if newline {
        let galley = layout(dir, false);
        if galley.rows.len() == 1 {
            return galley;
        }
    }
    let mut last = None;
    for shown in &dirs {
        let galley = layout(shown, newline);
        if !galley.elided {
            return galley;
        }
        last = Some(galley);
    }
    if newline {
        // 文件名本身就放不下一行：不显示目录，让文件名占满所有行。
        return layout("", false);
    }
    last.unwrap_or_else(|| layout(dir, false))
}

/// 等比缩小 `size` 放进 `bounds`（不放大）。
pub(super) fn fit(size: Vec2, bounds: Vec2) -> Vec2 {
    if size.x <= 0.0 || size.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.x / size.x).min(bounds.y / size.y).min(1.0);
    size * scale
}
