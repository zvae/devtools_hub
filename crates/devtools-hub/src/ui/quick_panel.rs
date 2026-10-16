//! 鼠标中键快捷面板：在鼠标处列出能处理选中内容的功能。
//!
//! 对应 uTools 的超级面板（7.x 中内部叫 voicePanel）：标题是“选中的文本 389 个”，
//! 下面列出匹配到的功能；没有选中内容时列出常用功能。

use egui::{Align, Align2, Color32, FontId, Key, Modifiers, Rect, Sense, Stroke, Ui, Vec2};

use super::fonts;
use super::icons::{self, Icon};
use super::style::Palette;
use super::HubApp;
use crate::selection::Selection;
use crate::tools::{self, QuickAction};

const WIDTH: f32 = 300.0;
/// 窗口内边距（与 `HubApp::draw` 中这一页的 inner_margin 一致）。
const MARGIN: f32 = 6.0;
const HEADER_HEIGHT: f32 = 46.0;
const HINT_HEIGHT: f32 = 26.0;
const ROW_HEIGHT: f32 = 40.0;
const ROW_GAP: f32 = 2.0;

#[derive(Default)]
pub struct QuickPanelView {
    pub selection: Option<Selection>,
    pub actions: Vec<QuickAction>,
    pub selected: usize,
}

impl QuickPanelView {
    pub fn open(&mut self, selection: Option<Selection>) {
        self.actions = tools::actions_for(selection.as_ref());
        self.selection = selection;
        self.selected = 0;
    }

    /// 选中了内容，但没有功能能处理它。
    fn unmatched(&self) -> bool {
        self.selection
            .as_ref()
            .is_some_and(|selection| !tools::has_match(selection))
    }

    /// 窗口大小随功能个数变化。
    pub fn window_size(&self) -> [f32; 2] {
        let hint = if self.unmatched() { HINT_HEIGHT } else { 0.0 };
        let rows = self.actions.len() as f32;
        [
            WIDTH,
            MARGIN * 2.0 + HEADER_HEIGHT + hint + rows * (ROW_HEIGHT + ROW_GAP) + 4.0,
        ]
    }
}

impl HubApp {
    pub(super) fn quick_ui(&mut self, ui: &mut Ui) {
        let palette = self.palette(ui.ctx());
        let count = self.quick.actions.len();
        let mut run = None;
        let mut close = false;
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
        ui.input_mut(|input| {
            if count > 0 {
                if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    self.quick.selected = (self.quick.selected + 1) % count;
                }
                if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    self.quick.selected = (self.quick.selected + count - 1) % count;
                }
                if input.consume_key(Modifiers::NONE, Key::Enter) {
                    run = Some(self.quick.selected);
                }
                for (index, key) in number_keys.iter().enumerate().take(count) {
                    if input.consume_key(Modifiers::NONE, *key) {
                        run = Some(index);
                    }
                }
            }
            close = input.consume_key(Modifiers::NONE, Key::Escape);
        });

        self.quick_header(ui, &palette);
        if self.quick.unmatched() {
            let (rect, _) = ui
                .allocate_exact_size(Vec2::new(ui.available_width(), HINT_HEIGHT), Sense::hover());
            ui.painter().text(
                rect.left_center() + Vec2::new(8.0, 0.0),
                Align2::LEFT_CENTER,
                "没有能处理选中内容的功能",
                FontId::proportional(12.0),
                palette.weak,
            );
        }
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        for index in 0..count {
            let action = self.quick.actions[index];
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
            if response.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                self.quick.selected = index;
            }
            if response.clicked() {
                run = Some(index);
            }
            action_row(
                ui,
                rect,
                action,
                index == self.quick.selected,
                index,
                &palette,
            );
        }

        if let Some(action) = run.and_then(|index| self.quick.actions.get(index).copied()) {
            self.run_quick_action(action);
        } else if close {
            self.hide_panel(true);
        }
    }

    fn quick_header(&self, ui: &mut Ui, palette: &Palette) {
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), HEADER_HEIGHT),
            Sense::hover(),
        );
        let (icon, title, detail) = match &self.quick.selection {
            Some(selection @ Selection::Text(text)) => (
                Icon::Document,
                selection.label(),
                devtools_clipboard::uti::single_line(text),
            ),
            Some(selection @ Selection::Files(paths)) => {
                let names: Vec<&str> = paths
                    .iter()
                    .map(|path| {
                        path.rsplit(['/', '\\'])
                            .find(|s| !s.is_empty())
                            .unwrap_or(path)
                    })
                    .collect();
                (Icon::Folder, selection.label(), names.join("、"))
            }
            None => (
                Icon::Keyboard,
                "常用功能".to_string(),
                "选中文字后按中键可直接处理它".to_string(),
            ),
        };
        let icon_rect = Rect::from_center_size(
            egui::pos2(rect.left() + 16.0, rect.center().y),
            Vec2::splat(16.0),
        );
        icons::paint(ui.painter(), icon_rect, icon, palette.weak);
        let text_left = rect.left() + 34.0;
        let width = rect.right() - text_left - 8.0;
        let title = ui.painter().layout_no_wrap(
            title,
            FontId::new(13.0, egui::FontFamily::Name(fonts::SEMIBOLD.into())),
            palette.text,
        );
        ui.painter()
            .galley(egui::pos2(text_left, rect.top() + 6.0), title, palette.text);
        let mut job = egui::text::LayoutJob::simple_singleline(
            detail,
            FontId::proportional(11.5),
            palette.weak,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(width);
        let detail = ui.painter().layout_job(job);
        ui.painter().galley(
            egui::pos2(text_left, rect.top() + 25.0),
            detail,
            palette.weak,
        );
        ui.painter().hline(
            rect.x_range().shrink(4.0),
            rect.bottom() - 0.5,
            Stroke::new(1.0, palette.card_stroke),
        );
    }

    /// 执行快捷面板中选中的功能。
    pub(super) fn run_quick_action(&mut self, action: QuickAction) {
        match action {
            QuickAction::JsonEditor => {
                let selection = self.quick.selection.take();
                self.open_json_editor(selection);
            }
            QuickAction::ClipboardHistory => self.open_clipboard_panel(),
        }
    }
}

fn action_row(
    ui: &Ui,
    rect: Rect,
    action: QuickAction,
    selected: bool,
    index: usize,
    palette: &Palette,
) {
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 8, palette.accent.gamma_multiply(0.16));
    }
    let badge_rect = Rect::from_center_size(
        egui::pos2(rect.left() + 22.0, rect.center().y),
        Vec2::splat(26.0),
    );
    paint_badge(ui, badge_rect, action, palette);
    painter.text(
        egui::pos2(rect.left() + 44.0, rect.center().y),
        Align2::LEFT_CENTER,
        action.label(),
        FontId::proportional(13.5),
        palette.text,
    );
    let hint = if selected {
        "↩".to_string()
    } else if index < 9 {
        (index + 1).to_string()
    } else {
        String::new()
    };
    let right = painter.text(
        egui::pos2(rect.right() - 12.0, rect.center().y),
        Align2::RIGHT_CENTER,
        hint,
        FontId::proportional(12.0),
        palette.weak,
    );
    let mut job = egui::text::LayoutJob::simple_singleline(
        action.description().to_string(),
        FontId::proportional(11.5),
        palette.weak,
    );
    let label_width = ui
        .painter()
        .layout_no_wrap(
            action.label().into(),
            FontId::proportional(13.5),
            palette.text,
        )
        .size()
        .x;
    let space = right.left() - (rect.left() + 44.0 + label_width) - 20.0;
    if space > 40.0 {
        job.wrap = egui::text::TextWrapping::truncate_at_width(space);
        job.halign = Align::RIGHT;
        let galley = painter.layout_job(job);
        painter.galley(
            egui::pos2(right.left() - 10.0, rect.center().y - galley.size().y / 2.0),
            galley,
            palette.weak,
        );
    }
}

/// 功能图标：圆角方块上画“{ }”或剪贴板图标（类似 uTools 插件的 logo）。
pub(super) fn paint_badge(ui: &Ui, rect: Rect, action: QuickAction, palette: &Palette) {
    let painter = ui.painter();
    match action {
        QuickAction::JsonEditor => {
            painter.rect_filled(rect, 6, Color32::from_rgb(0x3a, 0x3d, 0x46));
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "{ }",
                FontId::monospace(rect.height() * 0.42),
                Color32::WHITE,
            );
        }
        QuickAction::ClipboardHistory => {
            painter.rect_filled(rect, 6, palette.accent);
            icons::paint_centered(
                painter,
                rect,
                rect.height() * 0.58,
                Icon::Document,
                Color32::WHITE,
            );
        }
    }
}
