//! 剪贴板历史面板。
//!
//! 布局参照 PasteNow：顶部搜索栏，分类标签，卡片式条目（左侧来源应用图标、
//! 中间内容预览、右上角时间、右下角 ⌘N 快捷键），底部显示条目总数。

use devtools_clipboard::store::now_ms;
use devtools_clipboard::{
    ClipItem, ClipList, ContentKind, ItemQuery, KindFilter, FAVORITES_LIST_ID,
};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Rect,
    Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use super::fonts;
use super::icons::{self, Icon};
use super::style::{self, kind_color, kind_icon, Palette};
use super::{HubApp, PasteMode};
use crate::hotkey;
use crate::platform::{keyboard, panel as native_panel, workspace};

const CARD_HEIGHT: f32 = 74.0;
const FAVORITE_COLOR: Color32 = Color32::from_rgb(0xf5, 0xa6, 0x23);
const CARD_GAP: f32 = 8.0;
const SIDE: f32 = 12.0;
const QUERY_LIMIT: usize = 1000;
/// 预览区最多显示的字符数，超长文本在 egui 里排版会明显变慢。
const PREVIEW_MAX_CHARS: usize = 20_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Textual,
    Kind(ContentKind),
    List(i64),
}

impl Filter {
    fn to_query(self) -> (Option<KindFilter>, Option<i64>) {
        match self {
            Filter::All => (None, None),
            Filter::Textual => (Some(KindFilter::Textual), None),
            Filter::Kind(kind) => (Some(KindFilter::Exact(kind)), None),
            Filter::List(id) => (None, Some(id)),
        }
    }
}

struct Preview {
    id: i64,
    text: String,
    truncated: bool,
    char_count: usize,
}

#[derive(Default)]
pub struct PanelView {
    pub query: String,
    pub filter: Filter,
    pub items: Vec<ClipItem>,
    pub lists: Vec<ClipList>,
    /// 当前条件下的条目总数（列表最多加载 `QUERY_LIMIT` 条）。
    pub total: i64,
    pub selected: usize,
    pub needs_reload: bool,
    pub seen_revision: u64,
    /// 固定窗口：失去焦点时不自动隐藏。
    pub pinned: bool,
    /// 空格 / ⌘Y 打开的大预览。
    pub preview_open: bool,
    focus_search: bool,
    scroll_to_selected: bool,
    scroll_offset: f32,
    viewport_height: f32,
    preview: Option<Preview>,
    new_list_name: String,
    error: Option<String>,
}

impl PanelView {
    pub fn reset_for_show(&mut self) {
        self.query.clear();
        self.filter = Filter::All;
        self.selected = 0;
        self.needs_reload = true;
        self.focus_search = true;
        self.scroll_to_selected = true;
        self.scroll_offset = 0.0;
        self.preview = None;
        self.preview_open = false;
    }

    fn selected_item(&self) -> Option<&ClipItem> {
        self.items.get(self.selected)
    }

    fn filters(&self) -> Vec<(String, Filter)> {
        let mut filters = vec![
            ("全部".to_string(), Filter::All),
            ("文本".to_string(), Filter::Textual),
            ("链接".to_string(), Filter::Kind(ContentKind::Link)),
            ("图片".to_string(), Filter::Kind(ContentKind::Image)),
            ("文件".to_string(), Filter::Kind(ContentKind::Files)),
        ];
        for list in &self.lists {
            let label = if list.id == FAVORITES_LIST_ID {
                format!("★ {}", list.name)
            } else {
                list.name.clone()
            };
            filters.push((label, Filter::List(list.id)));
        }
        filters
    }

    fn cycle_filter(&mut self, forward: bool) {
        let filters = self.filters();
        let current = filters
            .iter()
            .position(|(_, f)| *f == self.filter)
            .unwrap_or(0);
        let len = filters.len();
        let next = if forward {
            (current + 1) % len
        } else {
            (current + len - 1) % len
        };
        self.set_filter(filters[next].1);
    }

    fn set_filter(&mut self, filter: Filter) {
        if self.filter != filter {
            self.filter = filter;
            self.selected = 0;
            self.scroll_to_selected = true;
            self.needs_reload = true;
        }
    }

    fn select(&mut self, index: usize) {
        if self.items.is_empty() {
            return;
        }
        self.selected = index.min(self.items.len() - 1);
        self.scroll_to_selected = true;
    }
}

/// 面板上产生的操作，绘制结束后统一执行，避免绘制过程中借用冲突。
enum Action {
    Paste(i64, PasteMode),
    Delete(i64),
    SetInList { item: i64, list: i64, member: bool },
    CreateList { name: String, add_item: Option<i64> },
    TogglePreview,
    TogglePause,
    OpenSettings,
    Close,
    OpenUrl(String),
    Reveal(String),
    RequestAccessibility,
}

impl HubApp {
    pub(super) fn panel_ui(&mut self, ui: &mut Ui) {
        let palette = self.palette(ui.ctx());
        let mut actions = Vec::new();
        self.panel_keys(ui.ctx(), &mut actions);

        egui::Panel::top("header")
            .show_separator_line(false)
            .frame(Frame::NONE.inner_margin(Margin {
                left: 12,
                right: 12,
                top: 12,
                bottom: 4,
            }))
            .show(ui, |ui| self.header(ui, &palette, &mut actions));
        egui::Panel::top("chips")
            .show_separator_line(false)
            .frame(Frame::NONE.inner_margin(Margin {
                left: 12,
                right: 8,
                top: 4,
                bottom: 8,
            }))
            .show(ui, |ui| self.chips(ui, &palette, &mut actions));
        if self.panel.needs_reload {
            self.reload_items();
        }
        egui::Panel::bottom("footer")
            .show_separator_line(false)
            .frame(Frame::NONE.inner_margin(Margin {
                left: 10,
                right: 10,
                top: 4,
                bottom: 6,
            }))
            .show(ui, |ui| self.footer(ui, &palette, &mut actions));
        egui::CentralPanel::default()
            .frame(Frame::NONE)
            .show(ui, |ui| {
                if let Some(error) = self.panel.error.clone() {
                    ui.label(RichText::new(error).color(palette.warn));
                }
                if self.settings.paste_to_active_app && !self.accessibility_ok {
                    accessibility_banner(ui, &palette, &mut actions);
                }
                if self.panel.preview_open && !self.panel.items.is_empty() {
                    self.preview_ui(ui, &palette, &mut actions);
                } else {
                    self.panel.preview_open = false;
                    self.card_list(ui, &palette, &mut actions);
                }
            });

        self.apply_actions(actions);
    }

    fn reload_items(&mut self) {
        self.panel.needs_reload = false;
        let (kind, list_id) = self.panel.filter.to_query();
        let query = ItemQuery {
            text: self.panel.query.clone(),
            kind,
            list_id,
            limit: QUERY_LIMIT,
        };
        let previous = self.panel.selected_item().map(|item| item.id);
        let loaded = self
            .store
            .items(&query)
            .and_then(|items| Ok((items, self.store.count(&query)?, self.store.lists()?)));
        match loaded {
            Ok((items, total, lists)) => {
                self.panel.items = items;
                self.panel.total = total;
                self.panel.lists = lists;
                self.panel.error = None;
            }
            Err(err) => self.panel.error = Some(format!("读取历史失败：{err:#}")),
        }
        if let Filter::List(id) = self.panel.filter {
            if !self.panel.lists.iter().any(|list| list.id == id) {
                self.panel.filter = Filter::All;
                self.panel.needs_reload = true;
            }
        }
        // 后台新增记录导致的刷新：尽量保持选中同一条目。
        if !self.panel.scroll_to_selected {
            if let Some(index) =
                previous.and_then(|id| self.panel.items.iter().position(|item| item.id == id))
            {
                self.panel.selected = index;
            }
        }
        self.panel.selected = self
            .panel
            .selected
            .min(self.panel.items.len().saturating_sub(1));
        self.panel.preview = None;
    }

    fn panel_keys(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let panel = &mut self.panel;
        let selected = panel
            .selected_item()
            .map(|item| (item.id, item.in_list(FAVORITES_LIST_ID)));
        let count = panel.items.len();
        let number_keys = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        let query_empty = panel.query.is_empty();
        let mut quick_paste = None;
        let mut movement: Option<isize> = None;
        let (mut jump_first, mut jump_last, mut escape, mut tab, mut back_tab, mut preview) =
            (false, false, false, false, false, false);

        ctx.input_mut(|input| {
            // 顺序很重要：Modifiers::NONE 也会匹配按住 ⇧ 的按键，所以先检查带修饰键的组合。
            if let Some((id, favorite)) = selected {
                if input.consume_key(Modifiers::SHIFT, Key::Enter) {
                    actions.push(Action::Paste(id, PasteMode::PlainText));
                } else if input.consume_key(Modifiers::COMMAND, Key::Enter) {
                    actions.push(Action::Paste(id, PasteMode::CopyOnly));
                } else if input.consume_key(Modifiers::NONE, Key::Enter) {
                    actions.push(Action::Paste(id, PasteMode::Original));
                }
                if input.consume_key(Modifiers::COMMAND, Key::Backspace)
                    || input.consume_key(Modifiers::COMMAND, Key::Delete)
                {
                    actions.push(Action::Delete(id));
                }
                if input.consume_key(Modifiers::COMMAND, Key::D) {
                    actions.push(Action::SetInList {
                        item: id,
                        list: FAVORITES_LIST_ID,
                        member: !favorite,
                    });
                }
                // 空格只在搜索框为空时用于预览，否则正常输入空格。
                preview = input.consume_key(Modifiers::COMMAND, Key::Y)
                    || (query_empty && input.consume_key(Modifiers::NONE, Key::Space));
            }
            if input.consume_key(Modifiers::COMMAND, Key::Comma) {
                actions.push(Action::OpenSettings);
            }
            for (index, key) in number_keys.iter().enumerate() {
                if input.consume_key(Modifiers::COMMAND, *key) {
                    quick_paste = Some(index);
                }
            }
            back_tab = input.consume_key(Modifiers::SHIFT, Key::Tab);
            tab = !back_tab && input.consume_key(Modifiers::NONE, Key::Tab);
            jump_first = input.consume_key(Modifiers::COMMAND, Key::ArrowUp);
            jump_last = input.consume_key(Modifiers::COMMAND, Key::ArrowDown);
            if input.consume_key(Modifiers::NONE, Key::ArrowUp)
                || input.consume_key(Modifiers::CTRL, Key::P)
            {
                movement = Some(-1);
            }
            if input.consume_key(Modifiers::NONE, Key::ArrowDown)
                || input.consume_key(Modifiers::CTRL, Key::N)
            {
                movement = Some(1);
            }
            if input.consume_key(Modifiers::NONE, Key::PageUp) {
                movement = Some(-5);
            }
            if input.consume_key(Modifiers::NONE, Key::PageDown) {
                movement = Some(5);
            }
            escape = input.consume_key(Modifiers::NONE, Key::Escape);
        });

        if preview {
            actions.push(Action::TogglePreview);
        }
        if let Some(item) = quick_paste.and_then(|index| panel.items.get(index)) {
            actions.push(Action::Paste(item.id, PasteMode::Original));
        }
        if tab || back_tab {
            panel.cycle_filter(tab);
        }
        if jump_first {
            panel.select(0);
        }
        if jump_last {
            panel.select(count.saturating_sub(1));
        }
        if let Some(delta) = movement {
            panel.select(panel.selected.saturating_add_signed(delta));
        }
        if escape {
            if panel.preview_open {
                panel.preview_open = false;
            } else if !panel.query.is_empty() {
                panel.query.clear();
                panel.selected = 0;
                panel.needs_reload = true;
            } else {
                actions.push(Action::Close);
            }
        }
    }

    fn header(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<Action>) {
        // 先注册拖动区域，后添加的控件会覆盖它，因此只有空白处能拖动窗口。
        let drag = ui.interact(ui.max_rect(), ui.id().with("drag-window"), Sense::drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        centered_row(ui, 30.0, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let pinned = self.panel.pinned;
            let pin = icon_button(ui, Icon::Pin, pinned, palette).on_hover_text(if pinned {
                "已固定：点击其他应用时不隐藏"
            } else {
                "固定窗口"
            });
            if pin.clicked() {
                self.panel.pinned = !pinned;
            }
            let search_width = ui.available_width() - 34.0;
            self.search_field(ui, search_width, palette);
            if icon_button(ui, Icon::Gear, false, palette)
                .on_hover_text("设置  ⌘,")
                .clicked()
            {
                actions.push(Action::OpenSettings);
            }
        });
    }

    fn search_field(&mut self, ui: &mut Ui, width: f32, palette: &Palette) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
        let id = Id::new("search");
        let focused = ui.memory(|memory| memory.has_focus(id));
        let stroke = if focused {
            Stroke::new(1.0, palette.accent.gamma_multiply(0.7))
        } else {
            Stroke::new(1.0, palette.card_stroke)
        };
        ui.painter()
            .rect(rect, 8, palette.field, stroke, StrokeKind::Inside);
        let icon_rect = Rect::from_center_size(
            egui::pos2(rect.left() + 16.0, rect.center().y),
            Vec2::splat(14.0),
        );
        icons::paint(ui.painter(), icon_rect, Icon::Search, palette.weak);
        let edit_rect = Rect::from_min_max(
            egui::pos2(rect.left() + 30.0, rect.top()),
            egui::pos2(rect.right() - 8.0, rect.bottom()),
        );
        let response = ui.put(
            edit_rect,
            egui::TextEdit::singleline(&mut self.panel.query)
                .id(id)
                .hint_text(RichText::new("输入开始搜索…").color(palette.weak))
                .font(FontId::proportional(14.0))
                .text_color(palette.text)
                .frame(Frame::NONE)
                .margin(Margin::ZERO)
                .min_size(edit_rect.size())
                .vertical_align(Align::Center)
                .desired_width(edit_rect.width())
                .lock_focus(true),
        );
        if response.changed() {
            self.panel.selected = 0;
            self.panel.scroll_to_selected = true;
            self.panel.preview_open = false;
            self.panel.needs_reload = true;
        }
        let wants_focus =
            self.panel.focus_search || (!response.has_focus() && !ui.ctx().any_popup_open());
        if wants_focus && (native_panel::is_visible() || cfg!(test)) {
            response.request_focus();
            self.panel.focus_search = false;
        }
    }

    fn chips(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<Action>) {
        let filters = self.panel.filters();
        centered_row(ui, 28.0, |ui| {
            let scroll_width = ui.available_width() - 30.0;
            egui::ScrollArea::horizontal()
                .id_salt("chips")
                .max_width(scroll_width)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    centered_row(ui, 28.0, |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (label, filter) in filters {
                            let selected = self.panel.filter == filter;
                            if chip(ui, &label, selected, palette).clicked() {
                                self.panel.set_filter(filter);
                            }
                        }
                    });
                });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let plus = icon_button(ui, Icon::Plus, false, palette).on_hover_text("新建列表");
                egui::Popup::menu(&plus).show(|ui| {
                    ui.label(RichText::new("新建列表").strong());
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.panel.new_list_name)
                            .hint_text("列表名称")
                            .desired_width(160.0),
                    );
                    edit.request_focus();
                    let submit = ui.input(|i| i.key_pressed(Key::Enter));
                    if (ui.button("创建").clicked() || submit)
                        && !self.panel.new_list_name.trim().is_empty()
                    {
                        actions.push(Action::CreateList {
                            name: std::mem::take(&mut self.panel.new_list_name),
                            add_item: None,
                        });
                        ui.close();
                    }
                });
            });
        });
    }

    fn footer(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<Action>) {
        centered_row(ui, 28.0, |ui| {
            let paused = self.settings.paused;
            let icon = if paused { Icon::Play } else { Icon::Pause };
            let pause = icon_button(ui, icon, paused, palette).on_hover_text(if paused {
                "已暂停记录，点击恢复"
            } else {
                "暂停记录"
            });
            if pause.clicked() {
                actions.push(Action::TogglePause);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                icon_button(ui, Icon::Keyboard, false, palette)
                    .on_hover_ui(|ui| self.shortcut_help(ui, palette));
                let count = style::group_digits(self.panel.total);
                let text = if paused {
                    RichText::new(format!("已暂停 · {count} 个项目")).color(palette.warn)
                } else {
                    RichText::new(format!("{count} 个项目")).color(palette.weak)
                };
                ui.centered_and_justified(|ui| ui.label(text.size(12.0)));
            });
        });
    }

    fn shortcut_help(&self, ui: &mut Ui, palette: &Palette) {
        let target = native_panel::previous_app_name()
            .map_or_else(|| "粘贴".to_string(), |app| format!("粘贴到 {app}"));
        let paste = if self.settings.paste_to_active_app {
            format!("{target}（或双击）")
        } else {
            "复制（或双击）".to_string()
        };
        let hotkey = hotkey::display_parts(&self.settings.hotkey);
        let rows: [(Vec<&str>, &str); 11] = [
            (vec!["↩"], paste.as_str()),
            (vec!["⇧", "↩"], "以纯文本粘贴"),
            (vec!["⌘", "↩"], "仅复制"),
            (vec!["⌘", "1–9"], "快速粘贴第 N 条"),
            (vec!["空格"], "预览（搜索框为空时）"),
            (vec!["⌘", "D"], "收藏 / 取消收藏"),
            (vec!["⌘", "⌫"], "删除"),
            (vec!["⇥"], "切换分类"),
            (vec!["⌘", ","], "设置"),
            (vec!["esc"], "清空搜索 / 关闭"),
            (
                hotkey.iter().map(String::as_str).collect(),
                "在任意应用中唤起",
            ),
        ];
        egui::Grid::new("shortcuts")
            .num_columns(2)
            .spacing([14.0, 6.0])
            .show(ui, |ui| {
                for (keys, label) in rows {
                    keycaps(ui, &keys, palette);
                    ui.label(RichText::new(label).color(palette.text));
                    ui.end_row();
                }
            });
    }

    fn card_list(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<Action>) {
        if self.panel.items.is_empty() {
            ui.add_space(ui.available_height() * 0.28);
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
            return;
        }

        ui.spacing_mut().item_spacing.y = CARD_GAP;
        let stride = CARD_HEIGHT + CARD_GAP;
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("items")
            .auto_shrink([false, false]);
        if std::mem::take(&mut self.panel.scroll_to_selected) {
            let top = self.panel.selected as f32 * stride;
            let bottom = top + CARD_HEIGHT;
            let view = self.panel.viewport_height.max(CARD_HEIGHT);
            if top < self.panel.scroll_offset {
                scroll = scroll.vertical_scroll_offset(top);
            } else if bottom > self.panel.scroll_offset + view {
                scroll = scroll.vertical_scroll_offset(bottom - view + CARD_GAP);
            }
        }
        let count = self.panel.items.len();
        let output = scroll.show_rows(ui, CARD_HEIGHT, count, |ui, range| {
            for index in range {
                let item = self.panel.items[index].clone();
                self.card(ui, index, &item, palette, actions);
            }
        });
        self.panel.scroll_offset = output.state.offset.y;
        self.panel.viewport_height = output.inner_rect.height();
    }

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
        painter.rect(
            rect,
            10,
            fill,
            Stroke::new(1.0, palette.card_stroke),
            StrokeKind::Inside,
        );
        if selected {
            painter.rect_stroke(
                rect,
                10,
                Stroke::new(2.0, palette.accent),
                StrokeKind::Inside,
            );
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
                painter.rect_filled(icon_rect, 8, color.gamma_multiply(0.16));
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
                format!("⌘{}", index + 1),
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
                        .corner_radius(4)
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
                let title = wrapped(
                    ui,
                    &item.title,
                    13.5,
                    palette.text,
                    content.right() - text_left,
                    1,
                );
                let subtitle = if item.file_count > 1 {
                    format!("{} 个文件", item.file_count)
                } else {
                    "文件".to_string()
                };
                let subtitle = wrapped(ui, &subtitle, 11.5, palette.weak, content.width(), 1);
                let title_height = title.size().y;
                let top = content.center().y - (title_height + 4.0 + subtitle.size().y) / 2.0;
                let icon = Rect::from_center_size(
                    egui::pos2(content.left() + 7.0, top + title_height / 2.0),
                    Vec2::splat(14.0),
                );
                icons::paint(&painter, icon, Icon::Document, palette.weak);
                painter.galley(egui::pos2(text_left, top), title, palette.text);
                painter.galley(
                    egui::pos2(text_left, top + title_height + 4.0),
                    subtitle,
                    palette.weak,
                );
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
                let galley = wrapped(ui, text, 13.5, color, content.width(), 2);
                let pos = egui::pos2(content.left(), content.center().y - galley.size().y / 2.0);
                painter.galley(pos, galley, color);
            }
        }

        response.context_menu(|ui| self.item_menu(ui, item, actions));
    }

    fn item_menu(&mut self, ui: &mut Ui, item: &ClipItem, actions: &mut Vec<Action>) {
        let target = native_panel::previous_app_name()
            .map_or_else(|| "粘贴".to_string(), |app| format!("粘贴到 {app}"));
        if ui.button(target).clicked() {
            actions.push(Action::Paste(item.id, PasteMode::Original));
        }
        if ui.button("以纯文本粘贴").clicked() {
            actions.push(Action::Paste(item.id, PasteMode::PlainText));
        }
        if ui.button("仅复制").clicked() {
            actions.push(Action::Paste(item.id, PasteMode::CopyOnly));
        }
        if ui.button("预览").clicked() {
            actions.push(Action::TogglePreview);
        }
        ui.separator();
        ui.menu_button("添加到列表", |ui| {
            for list in self.panel.lists.clone() {
                let mut member = item.in_list(list.id);
                if ui.checkbox(&mut member, &list.name).changed() {
                    actions.push(Action::SetInList {
                        item: item.id,
                        list: list.id,
                        member,
                    });
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.panel.new_list_name)
                        .hint_text("新列表名称")
                        .desired_width(120.0),
                );
                let submit = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if (ui.button("新建").clicked() || submit)
                    && !self.panel.new_list_name.trim().is_empty()
                {
                    actions.push(Action::CreateList {
                        name: std::mem::take(&mut self.panel.new_list_name),
                        add_item: Some(item.id),
                    });
                    ui.close();
                }
            });
        });
        if item.kind == ContentKind::Files {
            let first_path = self
                .store
                .text(item.id)
                .ok()
                .flatten()
                .and_then(|text| text.lines().next().map(str::to_string));
            if let Some(path) = first_path {
                if ui.button("在访达中显示").clicked() {
                    actions.push(Action::Reveal(path));
                }
            }
        }
        ui.separator();
        if ui
            .button(RichText::new("删除").color(ui.visuals().error_fg_color))
            .clicked()
        {
            actions.push(Action::Delete(item.id));
        }
    }

    fn preview_ui(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<Action>) {
        let Some(item) = self.panel.selected_item().cloned() else {
            return;
        };
        if self.panel.preview.as_ref().is_none_or(|p| p.id != item.id) {
            let full = self.store.text(item.id).ok().flatten().unwrap_or_default();
            let char_count = full.chars().count();
            let truncated = char_count > PREVIEW_MAX_CHARS;
            let text = if truncated {
                full.chars().take(PREVIEW_MAX_CHARS).collect()
            } else {
                full
            };
            self.panel.preview = Some(Preview {
                id: item.id,
                text,
                truncated,
                char_count,
            });
        }
        // 暂时取出预览，绘制时需要可变借用 self（纹理缓存），结束后放回。
        let Some(preview) = self.panel.preview.take() else {
            return;
        };

        Frame::NONE
            .inner_margin(Margin::symmetric(12, 0))
            .show(ui, |ui| {
                let favorite = item.in_list(FAVORITES_LIST_ID);
                centered_row(ui, 30.0, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if icon_button(ui, Icon::Back, false, palette).clicked() {
                        actions.push(Action::TogglePreview);
                    }
                    let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    icons::paint(
                        ui.painter(),
                        icon_rect,
                        kind_icon(item.kind),
                        kind_color(item.kind),
                    );
                    ui.label(
                        RichText::new(item.kind.label())
                            .color(kind_color(item.kind))
                            .family(egui::FontFamily::Name(fonts::SEMIBOLD.into())),
                    );
                    let detail = match item.kind {
                        ContentKind::Image => item
                            .image_size
                            .map(|(w, h)| format!("{w} × {h}"))
                            .unwrap_or_default(),
                        ContentKind::Files => format!("{} 个文件", item.file_count),
                        _ => format!("{} 个字符", preview.char_count),
                    };
                    ui.label(RichText::new(detail).color(palette.weak).size(12.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let icon = if favorite {
                            Icon::StarFilled
                        } else {
                            Icon::Star
                        };
                        let star =
                            icon_button(ui, icon, favorite, palette).on_hover_text("收藏  ⌘D");
                        if star.clicked() {
                            actions.push(Action::SetInList {
                                item: item.id,
                                list: FAVORITES_LIST_ID,
                                member: !favorite,
                            });
                        }
                    });
                });
                ui.add_space(4.0);

                // 底部：元信息与操作按钮；中间剩余空间给内容。
                egui::Panel::bottom("preview-actions")
                    .show_separator_line(false)
                    .frame(Frame::NONE.inner_margin(Margin {
                        top: 8,
                        bottom: 8,
                        ..Default::default()
                    }))
                    .show(ui, |ui| {
                        egui::Grid::new("meta")
                            .num_columns(2)
                            .spacing([12.0, 2.0])
                            .show(ui, |ui| {
                                let mut row = |key: &str, value: String| {
                                    ui.label(RichText::new(key).color(palette.weak).size(11.5));
                                    ui.label(RichText::new(value).color(palette.text).size(11.5));
                                    ui.end_row();
                                };
                                row("来源", item.source.display_name().to_string());
                                row("复制时间", style::absolute_time(item.last_used_at_ms));
                                row(
                                    "次数",
                                    format!(
                                        "复制 {} 次 · 粘贴 {} 次",
                                        item.copy_count, item.paste_count
                                    ),
                                );
                                row("大小", style::byte_size(item.byte_size.max(0) as u64));
                            });
                        ui.add_space(8.0);
                        ui.columns(2, |columns| {
                            let paste =
                                egui::Button::new(RichText::new("粘贴").color(Color32::WHITE))
                                    .fill(palette.accent)
                                    .min_size(Vec2::new(columns[0].available_width(), 28.0));
                            if columns[0].add(paste).on_hover_text("↩").clicked() {
                                actions.push(Action::Paste(item.id, PasteMode::Original));
                            }
                            let plain = egui::Button::new("以纯文本粘贴")
                                .min_size(Vec2::new(columns[1].available_width(), 28.0));
                            if columns[1].add(plain).on_hover_text("⇧↩").clicked() {
                                actions.push(Action::Paste(item.id, PasteMode::PlainText));
                            }
                        });
                    });

                Frame::new()
                    .fill(palette.card)
                    .stroke(Stroke::new(1.0, palette.card_stroke))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        match item.kind {
                            ContentKind::Image => self.image_preview(ui, &item),
                            ContentKind::Files => {
                                files_preview(ui, &preview.text, palette, actions)
                            }
                            ContentKind::Link => {
                                ui.label(RichText::new(&preview.text).color(palette.accent));
                                ui.add_space(8.0);
                                if ui.button("在浏览器中打开").clicked() {
                                    actions.push(Action::OpenUrl(preview.text.trim().to_string()));
                                }
                            }
                            _ => {
                                egui::ScrollArea::both()
                                    .id_salt("preview-text")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        let text = RichText::new(&preview.text)
                                            .monospace()
                                            .size(12.5)
                                            .color(palette.text);
                                        ui.add(egui::Label::new(text).extend());
                                        if preview.truncated {
                                            let note =
                                                format!("…… 仅显示前 {PREVIEW_MAX_CHARS} 个字符");
                                            ui.label(
                                                RichText::new(note).color(palette.weak).italics(),
                                            );
                                        }
                                    });
                            }
                        }
                    });
            });
        self.panel.preview = Some(preview);
    }

    fn image_preview(&mut self, ui: &mut Ui, item: &ClipItem) {
        match self.textures.get(ui.ctx(), &self.store, item.id) {
            Some(texture) => {
                let size = fit(texture.size_vec2(), ui.available_size());
                ui.centered_and_justified(|ui| {
                    ui.add(
                        egui::Image::from_texture(&texture)
                            .fit_to_exact_size(size)
                            .corner_radius(6),
                    );
                });
            }
            None => {
                ui.label(RichText::new("无法生成该图片格式的预览，粘贴不受影响").weak());
            }
        }
    }

    fn apply_actions(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Paste(id, mode) => self.paste(id, mode),
                Action::Delete(id) => {
                    if let Err(err) = self.store.delete(id) {
                        self.show_toast(format!("删除失败：{err:#}"));
                    }
                    self.textures.forget(id);
                    self.panel.needs_reload = true;
                }
                Action::SetInList { item, list, member } => {
                    if let Err(err) = self.store.set_in_list(item, list, member) {
                        self.show_toast(format!("更新列表失败：{err:#}"));
                    }
                    self.panel.needs_reload = true;
                }
                Action::CreateList { name, add_item } => {
                    let result = self.store.create_list(&name).and_then(|list| {
                        if let Some(item) = add_item {
                            self.store.set_in_list(item, list, true)?;
                        }
                        Ok(list)
                    });
                    match result {
                        Ok(list) => {
                            self.show_toast(format!("已创建列表“{}”", name.trim()));
                            if add_item.is_none() {
                                self.panel.set_filter(Filter::List(list));
                            }
                        }
                        Err(err) => self.show_toast(format!("{err:#}")),
                    }
                    self.panel.needs_reload = true;
                }
                Action::TogglePreview => {
                    self.panel.preview_open =
                        !self.panel.preview_open && !self.panel.items.is_empty();
                }
                Action::TogglePause => {
                    self.settings.paused = !self.settings.paused;
                    self.persist_settings();
                }
                Action::OpenSettings => {
                    self.settings_view.refresh(&self.store);
                    self.page = super::Page::Settings;
                }
                Action::Close => self.hide_panel(true),
                Action::OpenUrl(url) => {
                    workspace::open_url(&url);
                    self.hide_panel(false);
                }
                Action::Reveal(path) => {
                    workspace::reveal_path(&path);
                    self.hide_panel(false);
                }
                Action::RequestAccessibility => {
                    keyboard::accessibility_trusted(true);
                    keyboard::open_accessibility_settings();
                    self.hide_panel(false);
                }
            }
        }
    }
}

/// 无边框图标按钮：悬停时显示浅色底，`active` 时用强调色。
pub(super) fn icon_button(ui: &mut Ui, icon: Icon, active: bool, palette: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    if response.hovered() || active {
        let fill = if active {
            palette.accent.gamma_multiply(0.16)
        } else {
            palette.field
        };
        ui.painter().rect_filled(rect, 7, fill);
    }
    let color = if active { palette.accent } else { palette.weak };
    icons::paint_centered(ui.painter(), rect, 15.0, icon, color);
    response
}

/// 固定高度、内容垂直居中的一行。
///
/// `ui.horizontal` 的初始高度是 `interact_size.y`，先放进去的控件按这个高度居中，
/// 后放进去的更高控件会把行撑高，两者的中线就错开了；固定行高可以避免这种错位。
pub(super) fn centered_row<R>(
    ui: &mut Ui,
    height: f32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_height(height);
            add_contents(ui)
        },
    )
    .inner
}

/// 键帽：把 `["⌘", "V"]` 显示成一个个圆角小方块。
///
/// 先算出总宽度再一次性分配，放在从右到左的布局里也能正确对齐。
/// ↩ 与 ⇥ 在 SF 中没有字形，改为矢量图标绘制。
pub(super) fn keycaps(ui: &mut Ui, keys: &[&str], palette: &Palette) -> Response {
    const HEIGHT: f32 = 20.0;
    const GAP: f32 = 3.0;
    let symbol_icon = |key: &str| match key {
        "↩" => Some(Icon::Return),
        "⇥" => Some(Icon::Tab),
        _ => None,
    };
    let galleys: Vec<_> = keys
        .iter()
        .map(|key| {
            (symbol_icon(key).is_none()).then(|| {
                ui.painter().layout_no_wrap(
                    (*key).to_string(),
                    FontId::proportional(11.5),
                    palette.text,
                )
            })
        })
        .collect();
    let widths: Vec<f32> = galleys
        .iter()
        .map(|galley| {
            galley
                .as_ref()
                .map_or(HEIGHT, |g| (g.size().x + 10.0).max(HEIGHT))
        })
        .collect();
    let total = widths.iter().sum::<f32>() + GAP * widths.len().saturating_sub(1) as f32;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(total, HEIGHT), Sense::hover());
    let mut x = rect.left();
    for ((key, galley), width) in keys.iter().zip(galleys).zip(widths) {
        let cap = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(width, HEIGHT));
        ui.painter().rect(
            cap,
            5,
            palette.card,
            Stroke::new(1.0, palette.border),
            StrokeKind::Inside,
        );
        match (galley, symbol_icon(key)) {
            (Some(galley), _) => {
                ui.painter()
                    .galley(cap.center() - galley.size() / 2.0, galley, palette.text);
            }
            (None, Some(icon)) => {
                icons::paint_centered(ui.painter(), cap, 12.0, icon, palette.text)
            }
            (None, None) => {}
        }
        x += width + GAP;
    }
    response
}

/// 分类标签（胶囊形）。
fn chip(ui: &mut Ui, label: &str, selected: bool, palette: &Palette) -> Response {
    let color = if selected {
        palette.accent
    } else {
        palette.text
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(13.0), color);
    let size = galley.size() + Vec2::new(20.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if selected {
        palette.accent.gamma_multiply(0.16)
    } else if response.hovered() {
        palette.field
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}

fn accessibility_banner(ui: &mut Ui, palette: &Palette, actions: &mut Vec<Action>) {
    Frame::NONE
        .outer_margin(Margin {
            left: 12,
            right: 12,
            top: 0,
            bottom: 8,
        })
        .inner_margin(Margin::symmetric(10, 6))
        .fill(palette.warn.gamma_multiply(0.14))
        .corner_radius(CornerRadius::same(8))
        .show(ui, |ui| {
            centered_row(ui, 20.0, |ui| {
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                icons::paint(ui.painter(), icon_rect, Icon::Warning, palette.warn);
                let text = RichText::new("未授权“辅助功能”，只会复制到剪贴板")
                    .color(palette.warn)
                    .size(12.0);
                ui.label(text);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.link(RichText::new("去授权").size(12.0)).clicked() {
                        actions.push(Action::RequestAccessibility);
                    }
                });
            });
        });
}

fn files_preview(ui: &mut Ui, text: &str, palette: &Palette, actions: &mut Vec<Action>) {
    egui::ScrollArea::vertical()
        .id_salt("preview-files")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for path in text.lines().filter(|line| !line.is_empty()) {
                let exists = std::path::Path::new(path).exists();
                centered_row(ui, 22.0, |ui| {
                    let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    let (icon, color) = if exists {
                        (Icon::Document, palette.weak)
                    } else {
                        (Icon::Warning, palette.warn)
                    };
                    icons::paint(ui.painter(), icon_rect, icon, color);
                    let label = egui::Label::new(RichText::new(path).monospace().size(12.0))
                        .sense(Sense::click());
                    let hint = if exists {
                        "点击在访达中显示"
                    } else {
                        "文件已不存在"
                    };
                    if ui.add(label).on_hover_text(hint).clicked() && exists {
                        actions.push(Action::Reveal(path.to_string()));
                    }
                });
            }
        });
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

fn fit(size: Vec2, bounds: Vec2) -> Vec2 {
    if size.x <= 0.0 || size.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.x / size.x).min(bounds.y / size.y).min(1.0);
    size * scale
}
