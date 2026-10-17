//! JSON 编辑器：在鼠标中键快捷面板里选择“JSON 编辑器”后打开。
//!
//! 功能对应 uTools「JSON 编辑器」插件底栏的按钮（提示文字依次为）：
//! “重新格式化「Alt + F」”、“压缩 JSON 并复制「Alt + C」”、“压缩转义 JSON 并复制「Alt + \」”、
//! “去除注释”，以及输入框“JS 过滤”。插件把过滤表达式当 JavaScript 执行，
//! 这里改成不执行代码的路径过滤（`.key.subkey`、`[0][1]`、`.list[].name`）。
//! 另外加了“去除转义”，编辑区带行号与语法高亮，格式错误时底栏显示行列号，点击跳到出错位置。

use std::hash::{Hash, Hasher};

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::text_edit::TextEditState;
use egui::{
    Align, Align2, Color32, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Rect, RichText,
    Sense, Stroke, TextBuffer, Ui, Vec2,
};

use super::fonts;
use super::icons::{self, Icon};
use super::quick_panel::paint_badge;
use super::style::{self, Palette};
use super::widgets::{centered_row, drag_window, icon_button};
use super::HubApp;
use crate::keys;
use crate::tools::json::{self, Indent, JsonError, TokenKind};
use crate::tools::QuickAction;

/// 超过这个大小不做语法高亮：egui 排版大量带格式的片段会明显变慢。
const HIGHLIGHT_MAX_BYTES: usize = 256 * 1024;
/// 编辑区与过滤结果的字号（等宽字体）。
const FONT_SIZE: f32 = 13.0;
/// 编辑区与过滤输入框的控件 id（跳转光标、判断焦点用）。
const EDITOR_ID: &str = "json-editor";
const FILTER_ID: &str = "json-filter";

/// 底栏左侧显示的校验结果。
#[derive(Clone, Debug)]
enum Status {
    Empty,
    Valid { lines: usize },
    Invalid(JsonError),
}

/// 语法高亮的缓存：文本与明暗主题都没变时直接复用排版任务。
struct Highlight {
    hash: u64,
    dark: bool,
    job: LayoutJob,
}

/// JSON 编辑器的状态。窗口收起后保留，下次未选中内容打开时接着编辑。
#[derive(Default)]
pub struct JsonEditorView {
    /// 编辑区的文本。
    pub text: String,
    /// 路径过滤表达式。
    pub filter: String,
    /// 固定窗口：失去焦点时不自动隐藏。
    pub pinned: bool,
    /// 打开时的提示（例如“已去除转义”），编辑后消失。
    note: Option<&'static str>,
    // 以下是按文本哈希缓存的计算结果，避免每帧重新解析。
    /// （文本哈希，校验结果）。
    status: Option<(u64, Status)>,
    highlight: Option<Highlight>,
    /// （文本哈希，过滤表达式，缩进，过滤结果）。
    filtered: Option<(u64, String, Indent, Result<String, String>)>,
    /// 下一帧把光标放到这个字节偏移处并滚动过去。
    jump_to: Option<usize>,
}

impl JsonEditorView {
    /// 载入新内容：合法的 JSON 自动格式化，转义过的 JSON 先去掉转义。
    pub fn load(&mut self, text: &str, indent: Indent) {
        let prepared = json::prepare(text, indent);
        self.text = prepared.text;
        self.note = prepared.note;
        self.filter.clear();
        self.jump_to = Some(0);
    }

    /// 打开编辑器但不改变内容（未选中内容时）。
    pub fn focus(&mut self) {
        self.jump_to.get_or_insert(0);
    }

    /// 用操作结果替换文本（内容变了才清掉打开时的提示）。
    fn set_text(&mut self, text: String) {
        if text != self.text {
            self.text = text;
            self.note = None;
        }
    }

    /// 当前文本的校验结果（文本不变时用缓存）。
    fn status(&mut self) -> Status {
        let hash = text_hash(&self.text);
        if let Some((cached, status)) = &self.status {
            if *cached == hash {
                return status.clone();
            }
        }
        let status = if self.text.trim().is_empty() {
            Status::Empty
        } else {
            match json::parse(&self.text) {
                Ok(_) => Status::Valid {
                    lines: self.text.lines().count().max(1),
                },
                Err(err) => Status::Invalid(err),
            }
        };
        self.status = Some((hash, status.clone()));
        status
    }

    /// 路径过滤的结果（文本、表达式、缩进都不变时用缓存）。
    fn filtered(&mut self, indent: Indent) -> Result<String, String> {
        let hash = text_hash(&self.text);
        if let Some((cached, filter, cached_indent, result)) = &self.filtered {
            if *cached == hash && *filter == self.filter && *cached_indent == indent {
                return result.clone();
            }
        }
        let result = json::filter(&self.text, &self.filter, indent);
        self.filtered = Some((hash, self.filter.clone(), indent, result.clone()));
        result
    }
}

/// 编辑器上产生的操作，绘制结束后统一执行。
enum EditorAction {
    /// 重新格式化。
    Format,
    /// 去除注释并格式化。
    StripComments,
    /// 去除一层转义并格式化。
    Unescape,
    /// 复制原文。
    Copy,
    /// 压缩成一行后复制。
    CopyMinified,
    /// 压缩、转义成字符串字面量后复制。
    CopyEscaped,
    /// 修改缩进设置，并按新缩进重新格式化。
    SetIndent(Indent),
    /// 把光标移到这个字节偏移处（出错位置）。
    Jump(usize),
    Close,
}

impl HubApp {
    /// 绘制编辑器：顶部标题栏，底部过滤框与操作按钮，中间是编辑区；有过滤表达式时右侧显示过滤结果。
    pub(super) fn json_editor_ui(&mut self, ui: &mut Ui) {
        let palette = self.palette(ui.ctx());
        let mut actions = Vec::new();
        json_keys(ui.ctx(), &mut actions);
        let status = self.json.status();

        egui::Panel::top("json-header")
            .show_separator_line(false)
            .frame(Frame::NONE.inner_margin(Margin {
                left: style::PAD,
                right: style::PAD,
                top: style::PAD,
                bottom: 8,
            }))
            .show(ui, |ui| self.json_header(ui, &palette, &mut actions));
        egui::Panel::bottom("json-footer")
            .show_separator_line(false)
            .frame(Frame::NONE.inner_margin(Margin {
                left: style::PAD,
                right: style::PAD,
                top: 8,
                bottom: style::PAD,
            }))
            .show(ui, |ui| {
                self.json_footer(ui, &palette, &status, &mut actions)
            });
        egui::CentralPanel::default()
            .frame(Frame::NONE)
            .show(ui, |ui| {
                if !self.json.filter.trim().is_empty() {
                    let indent = self.settings.json_indent;
                    let result = self.json.filtered(indent);
                    egui::Panel::right("json-filter-result")
                        .resizable(true)
                        .default_size(ui.available_width() * 0.42)
                        .size_range(160.0..=ui.available_width() * 0.75)
                        .frame(
                            Frame::NONE
                                .fill(palette.field)
                                .inner_margin(Margin::symmetric(10, 8)),
                        )
                        .show(ui, |ui| filter_result(ui, &result, &palette));
                }
                self.json_text_area(ui, &palette, &status);
            });

        self.apply_editor_actions(ui.ctx(), actions);
    }

    /// 标题栏：图标、标题、打开时的提示，右侧是关闭与固定按钮；空白处可以拖动窗口。
    fn json_header(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<EditorAction>) {
        drag_window(ui, ui.max_rect());
        centered_row(ui, 28.0, |ui| {
            let (badge, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
            paint_badge(ui, badge, QuickAction::JsonEditor, palette);
            ui.label(
                RichText::new("JSON 编辑器")
                    .size(14.0)
                    .family(egui::FontFamily::Name(fonts::SEMIBOLD.into()))
                    .color(palette.text),
            );
            if let Some(note) = self.json.note {
                ui.label(RichText::new(note).size(11.5).color(palette.weak));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if icon_button(ui, Icon::Close, false, palette)
                    .on_hover_text(format!("关闭  {}", keys::ESCAPE))
                    .clicked()
                {
                    actions.push(EditorAction::Close);
                }
                let pinned = self.json.pinned;
                let pin = icon_button(ui, Icon::Pin, pinned, palette).on_hover_text(if pinned {
                    "已固定：点击其他应用时不隐藏"
                } else {
                    "固定窗口"
                });
                if pin.clicked() {
                    self.json.pinned = !pinned;
                }
            });
        });
    }

    /// 底栏：第一行是路径过滤输入框，第二行是校验状态、缩进选择与各操作按钮。
    fn json_footer(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        status: &Status,
        actions: &mut Vec<EditorAction>,
    ) {
        // 第一行：路径过滤（对应 uTools 插件底栏的 “this” + “JS 过滤”输入框）。
        centered_row(ui, 28.0, |ui| {
            let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
            let id = Id::new(FILTER_ID);
            let focused = ui.memory(|memory| memory.has_focus(id));
            let stroke = if focused {
                Stroke::new(1.0, palette.accent.gamma_multiply(0.7))
            } else {
                Stroke::new(1.0, palette.card_stroke)
            };
            ui.painter().rect(
                rect,
                style::CAPSULE,
                palette.field,
                stroke,
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                egui::pos2(rect.left() + 14.0, rect.center().y),
                Align2::LEFT_CENTER,
                "this",
                FontId::monospace(12.5),
                palette.weak,
            );
            let edit_rect = Rect::from_min_max(
                egui::pos2(rect.left() + 50.0, rect.top()),
                egui::pos2(rect.right() - 14.0, rect.bottom()),
            );
            let mut edit_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(edit_rect)
                    .layout(Layout::centered_and_justified(egui::Direction::TopDown)),
            );
            edit_ui.add(
                egui::TextEdit::singleline(&mut self.json.filter)
                    .id(id)
                    .hint_text(
                        RichText::new("路径过滤，例如 .key.subkey、[0][1]、.list[].name")
                            .color(palette.weak),
                    )
                    .font(FontId::monospace(12.5))
                    .text_color(palette.text)
                    .frame(Frame::NONE)
                    .margin(Margin::ZERO)
                    .vertical_align(Align::Center)
                    .desired_width(edit_rect.width()),
            );
        });
        ui.add_space(6.0);
        // 第二行：状态与操作按钮，和上面的输入框一样高。
        centered_row(ui, 28.0, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.spacing_mut().interact_size.y = 28.0;
                let alt = |key: &str| keys::shortcut(&[keys::ALT, key]);
                if ui
                    .button("转义复制")
                    .on_hover_text(format!("压缩、转义后复制  {}", alt("\\")))
                    .clicked()
                {
                    actions.push(EditorAction::CopyEscaped);
                }
                if ui
                    .button("压缩复制")
                    .on_hover_text(format!("压缩成一行后复制  {}", alt("C")))
                    .clicked()
                {
                    actions.push(EditorAction::CopyMinified);
                }
                if ui.button("复制").clicked() {
                    actions.push(EditorAction::Copy);
                }
                ui.add_space(6.0);
                if ui
                    .button("去除转义")
                    .on_hover_text("把 {\\\"a\\\":1} 这样转义过的文本还原")
                    .clicked()
                {
                    actions.push(EditorAction::Unescape);
                }
                if ui.button("去除注释").clicked() {
                    actions.push(EditorAction::StripComments);
                }
                if ui
                    .button("格式化")
                    .on_hover_text(format!("重新格式化  {}", alt("F")))
                    .clicked()
                {
                    actions.push(EditorAction::Format);
                }
                let mut indent = self.settings.json_indent;
                egui::ComboBox::from_id_salt("json-indent")
                    .width(74.0)
                    .selected_text(indent.label())
                    .show_ui(ui, |ui| {
                        for option in Indent::ALL {
                            ui.selectable_value(&mut indent, option, option.label());
                        }
                    });
                if indent != self.settings.json_indent {
                    actions.push(EditorAction::SetIndent(indent));
                }
                ui.add_space(6.0);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    status_label(ui, status, palette, actions);
                });
            });
        });
    }

    /// 编辑区：左侧行号栏（出错的行标红），右侧是带语法高亮、不折行的多行输入框。
    fn json_text_area(&mut self, ui: &mut Ui, palette: &Palette, status: &Status) {
        let ctx = ui.ctx().clone();
        let dark = ui.visuals().dark_mode;
        let font = FontId::monospace(FONT_SIZE);
        let editor_id = Id::new(EDITOR_ID);
        let JsonEditorView {
            text,
            highlight,
            jump_to,
            note,
            ..
        } = &mut self.json;

        // 先排版一次当前文本，用来确定行号栏宽度和编辑区大小（不折行，横向滚动）。
        let galley = ui.fonts_mut(|f| f.layout_job(highlighted(highlight, text, dark, &font)));
        let line_count = galley.rows.len().max(1);
        let digits = line_count.to_string().len().max(2) as f32;
        let digit_width = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let gutter_width = digits * digit_width + 22.0;
        let margin = Margin::symmetric(8, 6);
        let available = ui.available_size();
        let min_size = Vec2::new(
            (galley.size().x + margin.sum().x + 24.0).max(available.x - gutter_width),
            (galley.size().y + margin.sum().y).max(available.y),
        );

        let jump = jump_to.take().map(|offset| {
            let index = text[..offset.min(text.len())].chars().count();
            let mut state = TextEditState::load(&ctx, editor_id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(index))));
            state.store(&ctx, editor_id);
            ctx.memory_mut(|memory| memory.request_focus(editor_id));
            index
        });

        let error_line = match status {
            Status::Invalid(err) => Some(err.line),
            _ => None,
        };
        egui::ScrollArea::both()
            .id_salt("json-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let (gutter, _) =
                        ui.allocate_exact_size(Vec2::new(gutter_width, min_size.y), Sense::hover());
                    let mut layouter = |ui: &Ui, buffer: &dyn TextBuffer, _wrap_width: f32| {
                        let job = highlighted(highlight, buffer.as_str(), dark, &font);
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                    let output = egui::TextEdit::multiline(text)
                        .id(editor_id)
                        .code_editor()
                        .font(font.clone())
                        .frame(Frame::NONE)
                        .margin(margin)
                        .min_size(min_size)
                        .desired_width(min_size.x)
                        .layouter(&mut layouter)
                        .show(ui);
                    if output.response.changed() {
                        *note = None;
                    }

                    // 行号：按编辑区实际排版出的每一行绘制，只画可见部分。
                    let painter = ui.painter();
                    painter.rect_filled(
                        Rect::from_min_max(gutter.min, egui::pos2(gutter.max.x, gutter.max.y)),
                        0,
                        palette.field.gamma_multiply(0.6),
                    );
                    let clip = ui.clip_rect();
                    for (index, row) in output.galley.rows.iter().enumerate() {
                        let row_rect = row.rect().translate(output.galley_pos.to_vec2());
                        if row_rect.bottom() < clip.top() || row_rect.top() > clip.bottom() {
                            continue;
                        }
                        let is_error = error_line == Some(index + 1);
                        if is_error {
                            painter.rect_filled(
                                Rect::from_x_y_ranges(
                                    gutter.left()..=clip.right(),
                                    row_rect.y_range(),
                                ),
                                0,
                                palette.warn.gamma_multiply(0.14),
                            );
                        }
                        painter.text(
                            egui::pos2(gutter.right() - 10.0, row_rect.top()),
                            Align2::RIGHT_TOP,
                            (index + 1).to_string(),
                            font.clone(),
                            if is_error { palette.warn } else { palette.weak },
                        );
                    }

                    if let Some(index) = jump {
                        let cursor = output
                            .galley
                            .pos_from_cursor(CCursor::new(index))
                            .translate(output.galley_pos.to_vec2());
                        ui.scroll_to_rect(
                            cursor.expand2(Vec2::new(60.0, 40.0)),
                            Some(Align::Center),
                        );
                    }
                });
            });
    }

    /// 执行编辑器上的操作。格式不对时提示错误并把光标移到出错位置。
    fn apply_editor_actions(&mut self, ctx: &egui::Context, actions: Vec<EditorAction>) {
        let indent = self.settings.json_indent;
        for action in actions {
            match action {
                EditorAction::Format => match json::format(&self.json.text, indent) {
                    Ok(text) => self.json.set_text(text),
                    Err(err) => self.json_error("无法格式化", &err),
                },
                EditorAction::StripComments => {
                    let stripped = json::strip_comments(&self.json.text);
                    let text = json::format(&stripped, indent).unwrap_or(stripped);
                    self.json.set_text(text);
                    self.show_toast("已去除注释");
                }
                EditorAction::Unescape => match json::unescape(&self.json.text) {
                    Some(text) => {
                        let text = json::format(&text, indent).unwrap_or(text);
                        self.json.set_text(text);
                        self.show_toast("已去除转义");
                    }
                    None => self.show_toast("不是转义过的文本"),
                },
                EditorAction::Copy => {
                    ctx.copy_text(self.json.text.clone());
                    self.show_toast("已复制");
                }
                EditorAction::CopyMinified => match json::minify(&self.json.text) {
                    Ok(text) => {
                        ctx.copy_text(text);
                        self.show_toast("已复制压缩后的 JSON");
                    }
                    Err(err) => self.json_error("无法压缩", &err),
                },
                EditorAction::CopyEscaped => match json::escape(&self.json.text) {
                    Ok(text) => {
                        ctx.copy_text(text);
                        self.show_toast("已复制压缩、转义后的 JSON");
                    }
                    Err(err) => self.json_error("无法转义", &err),
                },
                EditorAction::SetIndent(new) => {
                    self.settings.json_indent = new;
                    self.persist_settings();
                    if let Ok(text) = json::format(&self.json.text, new) {
                        self.json.set_text(text);
                    }
                }
                EditorAction::Jump(offset) => self.json.jump_to = Some(offset),
                EditorAction::Close => self.hide_panel(true),
            }
        }
    }

    /// 提示操作失败的原因，并跳到出错位置。
    fn json_error(&mut self, what: &str, err: &JsonError) {
        self.show_toast(format!("{what}：{err}"));
        self.json.jump_to = Some(err.offset);
    }
}

/// 编辑器快捷键，与 uTools 插件一致：⌥F 重新格式化、⌥C 压缩复制、⌥\ 转义复制；esc 关闭。
fn json_keys(ctx: &egui::Context, actions: &mut Vec<EditorAction>) {
    ctx.input_mut(|input| {
        let mut consumed_alt = false;
        for (key, action) in [
            (Key::F, EditorAction::Format),
            (Key::C, EditorAction::CopyMinified),
            (Key::Backslash, EditorAction::CopyEscaped),
        ] {
            if input.consume_key(Modifiers::ALT, key) {
                actions.push(action);
                consumed_alt = true;
            }
        }
        if consumed_alt {
            // macOS 上 ⌥F 还会产生“ƒ”之类的字符输入，不能让它进入编辑区。
            input
                .events
                .retain(|event| !matches!(event, egui::Event::Text(_)));
        }
        if input.consume_key(Modifiers::NONE, Key::Escape) {
            actions.push(EditorAction::Close);
        }
    });
}

/// 校验状态：图标加一行文字；格式错误时可以点击跳到出错位置。
fn status_label(ui: &mut Ui, status: &Status, palette: &Palette, actions: &mut Vec<EditorAction>) {
    let (icon, color, text) = match status {
        Status::Empty => (Icon::Document, palette.weak, "粘贴或输入 JSON".to_string()),
        Status::Valid { lines } => (
            Icon::Check,
            palette.ok,
            format!("JSON 格式正确 · {lines} 行"),
        ),
        Status::Invalid(err) => (Icon::Warning, palette.warn, err.to_string()),
    };
    let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    icons::paint(ui.painter(), icon_rect, icon, color);
    let label = egui::Label::new(RichText::new(text).size(12.0).color(color))
        .truncate()
        .sense(Sense::click());
    let response = ui.add(label);
    if let Status::Invalid(err) = status {
        if response.on_hover_text("点击跳到出错位置").clicked() {
            actions.push(EditorAction::Jump(err.offset));
        }
    }
}

/// 右侧的过滤结果（同样做语法高亮），出错时显示原因。
fn filter_result(ui: &mut Ui, result: &Result<String, String>, palette: &Palette) {
    ui.label(RichText::new("过滤结果").size(11.5).color(palette.weak));
    ui.add_space(2.0);
    let dark = ui.visuals().dark_mode;
    egui::ScrollArea::both()
        .id_salt("json-filter-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| match result {
            Ok(text) => {
                let job = layout_job(text, dark, &FontId::monospace(FONT_SIZE));
                ui.add(egui::Label::new(job).extend());
            }
            Err(message) => {
                ui.label(RichText::new(message).color(palette.warn));
            }
        });
}

/// 文本的哈希，用作各个缓存的键。
fn text_hash(text: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// 语法高亮（不折行），按文本和明暗主题缓存。
fn highlighted(cache: &mut Option<Highlight>, text: &str, dark: bool, font: &FontId) -> LayoutJob {
    let hash = text_hash(text);
    if let Some(cached) = cache {
        if cached.hash == hash && cached.dark == dark {
            return cached.job.clone();
        }
    }
    let job = layout_job(text, dark, font);
    *cache = Some(Highlight {
        hash,
        dark,
        job: job.clone(),
    });
    job
}

/// 配色取自 VS Code / Monaco 的默认主题（uTools 插件用的就是 Monaco）。
fn layout_job(text: &str, dark: bool, font: &FontId) -> LayoutJob {
    let (plain, key, string, number, literal, comment, invalid) = if dark {
        (
            Color32::from_rgb(0xd4, 0xd4, 0xd4),
            Color32::from_rgb(0x9c, 0xdc, 0xfe),
            Color32::from_rgb(0xce, 0x91, 0x78),
            Color32::from_rgb(0xb5, 0xce, 0xa8),
            Color32::from_rgb(0x56, 0x9c, 0xd6),
            Color32::from_rgb(0x6a, 0x99, 0x55),
            Color32::from_rgb(0xf4, 0x87, 0x71),
        )
    } else {
        (
            Color32::from_rgb(0x1f, 0x1f, 0x1f),
            Color32::from_rgb(0xa3, 0x15, 0x15),
            Color32::from_rgb(0x04, 0x51, 0xa5),
            Color32::from_rgb(0x09, 0x86, 0x58),
            Color32::from_rgb(0x00, 0x00, 0xff),
            Color32::from_rgb(0x00, 0x80, 0x00),
            Color32::from_rgb(0xcd, 0x31, 0x31),
        )
    };
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let format = |color: Color32| TextFormat::simple(font.clone(), color);
    if text.len() > HIGHLIGHT_MAX_BYTES {
        job.append(text, 0.0, format(plain));
        return job;
    }
    let tokens = json::tokenize(text);
    // 相邻同色的单元合并成一段，减少排版的片段数。
    let mut run: Option<(usize, Color32)> = None;
    for (index, token) in tokens.iter().enumerate() {
        let color = match token.kind {
            TokenKind::String => {
                // 后面（跳过空白和注释）紧跟冒号的字符串是键。
                let is_key = tokens[index + 1..]
                    .iter()
                    .find(|t| !t.is_trivia())
                    .is_some_and(|t| t.kind == TokenKind::Punct(b':'));
                if is_key {
                    key
                } else {
                    string
                }
            }
            TokenKind::Number => number,
            TokenKind::Word if matches!(token.text(text), "true" | "false" | "null") => literal,
            TokenKind::Word | TokenKind::Invalid => invalid,
            TokenKind::LineComment | TokenKind::BlockComment => comment,
            TokenKind::Punct(_) | TokenKind::Whitespace => plain,
        };
        match run {
            Some((_, run_color)) if run_color == color => {}
            Some((start, run_color)) => {
                job.append(&text[start..token.start], 0.0, format(run_color));
                run = Some((token.start, color));
            }
            None => run = Some((token.start, color)),
        }
    }
    if let Some((start, color)) = run {
        job.append(&text[start..], 0.0, format(color));
    }
    job
}
