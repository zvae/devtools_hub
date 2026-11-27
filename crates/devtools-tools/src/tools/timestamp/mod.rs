//! 内置时间戳工具：实时时钟、秒／毫秒与北京时间双向转换，选中内容直接带入。

use chrono::Utc;
use egui::{
    Align, Align2, Color32, FontId, Frame, Key, Layout, Margin, Modifiers, Rect, RichText, Sense,
    Stroke, Ui, Vec2,
};

use crate::support::drag_window;
use crate::support::icons::Icon;
use crate::support::widgets::{centered_row, icon_button};
use crate::support::{icons, style, widgets};
use devtools_module_api::Selection;
mod model;
use self::model::{self as timestamp, Unit};

pub(super) struct TimestampView {
    pub pinned: bool,
    pub date_to_timestamp: bool,
    pub running: bool,
    pub now_ms: i64,
    pub clock_unit: Unit,
    pub input: String,
    pub input_unit: Unit,
    pub date: String,
    pub output_unit: Unit,
}

impl Default for TimestampView {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            pinned: false,
            date_to_timestamp: false,
            running: true,
            now_ms: now.timestamp_millis(),
            clock_unit: Unit::Milliseconds,
            input: now.timestamp_millis().to_string(),
            input_unit: Unit::Milliseconds,
            date: timestamp::format_date(now),
            output_unit: Unit::Milliseconds,
        }
    }
}

impl TimestampView {
    /// 仅覆盖与选中内容对应的输入，另一个方向保留上次编辑的内容。
    pub fn load(&mut self, text: &str) {
        if let Some(unit) = timestamp::detect(text) {
            self.date_to_timestamp = false;
            self.input = text.trim().into();
            self.input_unit = unit;
        } else if timestamp::from_date(text).is_ok() {
            self.date_to_timestamp = true;
            self.date = text.trim().into();
        }
    }
}

impl Tool {
    pub(super) fn timestamp_ui(&mut self, ui: &mut Ui) {
        let palette = self.palette(ui.ctx());
        let mut close = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        Frame::NONE
            .inner_margin(Margin::symmetric(style::PAD, 12))
            .show(ui, |ui| {
                drag_window(ui, ui.max_rect(), &mut self.effects);
                centered_row(ui, 28.0, |ui| {
                    #[cfg(target_os = "macos")]
                    {
                        close |= widgets::traffic_light_close(ui).clicked();
                        ui.add_space(4.0);
                    }
                    ui.label(
                        RichText::new("时间戳转换")
                            .size(16.0)
                            .strong()
                            .color(palette.text),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        #[cfg(not(target_os = "macos"))]
                        {
                            close |= icon_button(ui, Icon::Close, false, &palette).clicked();
                        }
                        if icon_button(ui, Icon::Pin, self.timestamp.pinned, &palette)
                            .on_hover_text("固定窗口，点击其他应用时保持显示")
                            .clicked()
                        {
                            self.timestamp.pinned = !self.timestamp.pinned;
                        }
                    });
                });
            });
        let view = &mut self.timestamp;
        if view.running {
            view.now_ms = Utc::now().timestamp_millis();
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut copied = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            Frame::NONE
                .inner_margin(Margin::symmetric(24, 14))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 16.0;
                    // 时钟使用独立的轻量区域，数字本身可点击，不再堆叠胶囊按钮。
                    Frame::new()
                        .fill(palette.card)
                        .corner_radius(14.0)
                        .stroke(Stroke::new(1.0, palette.border))
                        .inner_margin(16.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 20.0),
                                Sense::hover(),
                            );
                            ui.painter().circle_filled(
                                rect.left_center() + Vec2::new(4.0, 0.0),
                                3.0,
                                if view.running {
                                    palette.ok
                                } else {
                                    palette.weak
                                },
                            );
                            ui.painter().text(
                                rect.left_center() + Vec2::new(16.0, 0.0),
                                Align2::LEFT_CENTER,
                                if view.running {
                                    "当前时间戳 · 实时"
                                } else {
                                    "当前时间戳 · 已暂停"
                                },
                                FontId::proportional(11.5),
                                palette.secondary,
                            );
                            let pause = Rect::from_center_size(
                                rect.right_center() - Vec2::new(30.0, 0.0),
                                Vec2::new(60.0, 28.0),
                            );
                            if ui
                                .put(
                                    pause,
                                    egui::Button::new(if view.running {
                                        "暂停"
                                    } else {
                                        "继续"
                                    })
                                    .fill(palette.field)
                                    .stroke(Stroke::new(1.0, palette.border))
                                    .corner_radius(7.0),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                view.running = !view.running;
                            }
                            let value = match view.clock_unit {
                                Unit::Seconds => view.now_ms.div_euclid(1000),
                                Unit::Milliseconds => view.now_ms,
                            }
                            .to_string();
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 40.0),
                                Sense::hover(),
                            );
                            let clock_size = if rect.width() < 400.0 { 18.0 } else { 28.0 };
                            let text_width = ui
                                .painter()
                                .layout_no_wrap(
                                    value.clone(),
                                    FontId::monospace(clock_size),
                                    palette.text,
                                )
                                .size()
                                .x;
                            let digits = Rect::from_min_size(
                                rect.min,
                                Vec2::new(text_width + 26.0, rect.height()),
                            );
                            if ui
                                .put(
                                    digits,
                                    egui::Button::new(
                                        RichText::new(&value).monospace().size(clock_size),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("点击复制并填入转换器")
                                .clicked()
                            {
                                view.input = value.clone();
                                view.input_unit = view.clock_unit;
                                view.date_to_timestamp = false;
                                self.effects.push(ToolEffect::Copy {
                                    text: value.clone(),
                                    sensitive: false,
                                });
                                copied = true;
                            }
                            let units = Rect::from_min_max(
                                egui::pos2(rect.right() - 104.0, rect.center().y - 14.0),
                                egui::pos2(rect.right(), rect.center().y + 14.0),
                            );
                            unit_switch(ui, units, &mut view.clock_unit, &palette);
                        });
                    // 一个转换器承载两个方向，输入、结果及单位始终处于固定位置。
                    Frame::new()
                        .fill(palette.card)
                        .corner_radius(14.0)
                        .stroke(Stroke::new(1.0, palette.border))
                        .inner_margin(18.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 12.0;
                            let (tabs, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 36.0),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(tabs, 8.0, palette.field);
                            let half = tabs.width() / 2.0;
                            for (index, label) in
                                ["时间戳 → 日期", "日期 → 时间戳"].into_iter().enumerate()
                            {
                                let rect = Rect::from_min_size(
                                    tabs.min + Vec2::new(index as f32 * half, 0.0),
                                    Vec2::new(half, 36.0),
                                )
                                .shrink(3.0);
                                let active = view.date_to_timestamp == (index == 1);
                                let button = egui::Button::new(
                                    RichText::new(label).size(12.0).color(if active {
                                        palette.text
                                    } else {
                                        palette.weak
                                    }),
                                )
                                .fill(if active {
                                    palette.card
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .corner_radius(6.0)
                                .stroke(Stroke::NONE);
                                if ui.put(rect, button).clicked() {
                                    view.date_to_timestamp = index == 1;
                                }
                            }
                            let (label, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 28.0),
                                Sense::hover(),
                            );
                            let date_mode = view.date_to_timestamp;
                            ui.painter().text(
                                label.left_center(),
                                Align2::LEFT_CENTER,
                                if date_mode {
                                    "日期与时间"
                                } else {
                                    "Unix 时间戳"
                                },
                                FontId::proportional(12.0),
                                palette.secondary,
                            );
                            let right = Rect::from_min_max(
                                label.right_top() - Vec2::new(104.0, 0.0),
                                label.right_bottom(),
                            );
                            if date_mode {
                                if ui
                                    .put(
                                        right,
                                        egui::Button::new(
                                            RichText::new("使用当前时间")
                                                .size(11.5)
                                                .color(palette.accent),
                                        )
                                        .frame(false),
                                    )
                                    .clicked()
                                {
                                    view.date = timestamp::format_date(Utc::now());
                                }
                            } else {
                                unit_switch(ui, right, &mut view.input_unit, &palette);
                            }
                            let (field, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 48.0),
                                Sense::hover(),
                            );
                            let id = ui.id().with(if date_mode {
                                "timestamp-date"
                            } else {
                                "timestamp-input"
                            });
                            let focus = ui.memory(|m| m.has_focus(id));
                            ui.painter().rect(
                                field,
                                8.0,
                                palette.field,
                                Stroke::new(
                                    1.0,
                                    if focus {
                                        palette.accent
                                    } else {
                                        palette.border
                                    },
                                ),
                                egui::StrokeKind::Inside,
                            );
                            let input = if date_mode {
                                &mut view.date
                            } else {
                                &mut view.input
                            };
                            // 显式设置编辑器高度与内部对齐；只缩小外层矩形不会改变单行文本的默认顶部对齐。
                            ui.put(
                                field,
                                egui::TextEdit::singleline(input)
                                    .id(id)
                                    .font(FontId::monospace(15.0))
                                    .frame(Frame::NONE.inner_margin(Margin::symmetric(12, 0)))
                                    .min_size(field.size())
                                    .vertical_align(Align::Center)
                                    .hint_text(if date_mode {
                                        "2026-11-23 12:30:00.000"
                                    } else {
                                        "输入秒或毫秒时间戳"
                                    }),
                            );
                            let (hint, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 16.0),
                                Sense::hover(),
                            );
                            icons::paint_centered(
                                ui.painter(),
                                Rect::from_center_size(
                                    hint.left_center() + Vec2::new(6.0, 0.0),
                                    Vec2::splat(12.0),
                                ),
                                12.0,
                                Icon::ChevronDown,
                                palette.weak,
                            );
                            ui.painter().text(
                                hint.left_center() + Vec2::new(22.0, 0.0),
                                Align2::LEFT_CENTER,
                                "自动转换",
                                FontId::proportional(11.0),
                                palette.weak,
                            );
                            let result = if date_mode {
                                timestamp::from_date(&view.date)
                                    .map(|date| view.output_unit.value(date).to_string())
                            } else {
                                timestamp::from_timestamp(&view.input, view.input_unit)
                                    .map(timestamp::format_date)
                            };
                            let (output, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 96.0),
                                Sense::hover(),
                            );
                            let tint = if palette.dark {
                                Color32::from_rgb(32, 43, 61)
                            } else {
                                Color32::from_rgb(240, 245, 255)
                            };
                            ui.painter().rect_filled(output, 10.0, tint);
                            let label_pos = output.min + Vec2::new(14.0, 19.0);
                            ui.painter().text(
                                label_pos,
                                Align2::LEFT_CENTER,
                                if date_mode {
                                    "转换结果 · Unix 时间戳"
                                } else {
                                    "转换结果 · 北京时间"
                                },
                                FontId::proportional(11.5),
                                palette.secondary,
                            );
                            if date_mode {
                                let units = Rect::from_min_size(
                                    egui::pos2(output.right() - 118.0, output.top() + 6.0),
                                    Vec2::new(104.0, 26.0),
                                );
                                unit_switch(ui, units, &mut view.output_unit, &palette);
                            } else {
                                ui.painter().text(
                                    output.right_top() + Vec2::new(-14.0, 19.0),
                                    Align2::RIGHT_CENTER,
                                    "UTC+08:00",
                                    FontId::monospace(10.5),
                                    palette.weak,
                                );
                            }
                            match result {
                                Ok(text) => {
                                    let copy = Rect::from_min_size(
                                        egui::pos2(output.right() - 70.0, output.top() + 47.0),
                                        Vec2::new(56.0, 32.0),
                                    );
                                    let text_rect = Rect::from_min_max(
                                        output.min + Vec2::new(14.0, 43.0),
                                        egui::pos2(copy.left() - 10.0, output.bottom() - 12.0),
                                    );
                                    let mut value = text.as_str();
                                    ui.put(
                                        text_rect,
                                        egui::TextEdit::singleline(&mut value)
                                            .frame(Frame::NONE)
                                            .min_size(text_rect.size())
                                            .vertical_align(Align::Center)
                                            .font(FontId::monospace(if output.width() < 400.0 {
                                                12.0
                                            } else {
                                                17.0
                                            }))
                                            .text_color(palette.text),
                                    );
                                    if ui
                                        .put(
                                            copy,
                                            egui::Button::new(
                                                RichText::new("复制")
                                                    .size(12.0)
                                                    .color(Color32::WHITE),
                                            )
                                            .fill(palette.accent)
                                            .stroke(Stroke::NONE)
                                            .corner_radius(7.0),
                                        )
                                        .clicked()
                                    {
                                        self.effects.push(ToolEffect::Copy {
                                            text,
                                            sensitive: false,
                                        });
                                        copied = true;
                                    }
                                }
                                Err(message) => {
                                    let rect = output.shrink2(Vec2::new(14.0, 8.0));
                                    let rect = Rect::from_min_max(
                                        rect.min + Vec2::new(0.0, 30.0),
                                        rect.max,
                                    );
                                    ui.put(
                                        rect,
                                        egui::Label::new(
                                            RichText::new(message).size(11.5).color(palette.danger),
                                        )
                                        .halign(Align::Min)
                                        .wrap(),
                                    );
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("北京时间 · UTC+08:00")
                                .size(11.0)
                                .color(palette.weak),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new("格式说明 ⓘ").size(11.0).color(palette.weak))
                                .on_hover_ui(|ui| format_help(ui, &palette));
                        });
                    });
                });
        });
        if copied {
            self.show_toast("已复制");
        }
        if close {
            self.hide_panel(true);
        }
    }
}

/// 两段式单位切换；固定尺寸保证输入区与结果区的操作始终对齐。
fn unit_switch(ui: &mut Ui, rect: Rect, unit: &mut Unit, palette: &style::Palette) {
    ui.painter().rect_filled(rect, 6.0, palette.field);
    for (index, (value, label)) in [(Unit::Seconds, "秒"), (Unit::Milliseconds, "毫秒")]
        .into_iter()
        .enumerate()
    {
        let segment = Rect::from_min_size(
            rect.min + Vec2::new(index as f32 * rect.width() / 2.0, 0.0),
            Vec2::new(rect.width() / 2.0, rect.height()),
        )
        .shrink(2.0);
        let active = *unit == value;
        if ui
            .put(
                segment,
                egui::Button::new(RichText::new(label).size(11.5).color(if active {
                    palette.accent
                } else {
                    palette.weak
                }))
                .fill(if active {
                    palette.card
                } else {
                    Color32::TRANSPARENT
                })
                .corner_radius(4.0)
                .stroke(Stroke::NONE),
            )
            .on_hover_text(value.label())
            .clicked()
        {
            *unit = value;
        }
    }
}

/// 按主题展示带标题、示例与分组的格式说明，限制宽度以免遮住整条结果。
fn format_help(ui: &mut Ui, palette: &style::Palette) {
    Frame::NONE.inner_margin(12.0).show(ui, |ui| {
        ui.set_width(268.0);
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.label(
            RichText::new("输入格式")
                .size(13.0)
                .strong()
                .color(palette.text),
        );
        help_section(
            ui,
            "Unix 时间戳",
            "支持秒、毫秒和负数，单位可手动切换。",
            "1700000000  /  1700000000123",
            palette,
        );
        ui.separator();
        help_section(
            ui,
            "日期与时间",
            "不带时区时按北京时间 UTC+08:00 解析。",
            "2026-11-23 12:30:00.123",
            palette,
        );
        ui.label(
            RichText::new("也支持带时区的 ISO 8601 日期。")
                .size(11.5)
                .color(palette.secondary),
        );
        ui.label(
            RichText::new("选中数字带入时：10 位以内默认按秒，更多位按毫秒。")
                .size(11.0)
                .color(palette.weak),
        );
    });
}

fn help_section(
    ui: &mut Ui,
    title: &str,
    description: &str,
    example: &str,
    palette: &style::Palette,
) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        ui.label(RichText::new(title).size(11.5).strong().color(palette.text));
        ui.label(
            RichText::new(description)
                .size(11.5)
                .color(palette.secondary),
        );
        Frame::new()
            .fill(palette.field)
            .corner_radius(5.0)
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    RichText::new(example)
                        .monospace()
                        .size(10.5)
                        .color(palette.text),
                );
            });
    });
}

use devtools_module_api::{ModuleDescriptor, ToolEffect, ToolModule};
use serde_json::Value;

struct Tool {
    timestamp: TimestampView,
    palette: style::Palette,
    effects: Vec<ToolEffect>,
}
crate::support::tool_helpers!(Tool);

fn create(_config: &Value) -> Box<dyn ToolModule> {
    Box::new(Tool {
        timestamp: TimestampView::default(),
        palette: style::Palette::new(true),
        effects: Vec::new(),
    })
}

fn paint_icon(ui: &Ui, rect: Rect, _palette: &style::Palette) {
    ui.painter().rect_filled(
        rect,
        style::SMALL_RADIUS,
        Color32::from_rgb(0x20, 0x92, 0x86),
    );
    icons::paint_centered(
        ui.painter(),
        rect,
        rect.height() * 0.62,
        Icon::Clock,
        Color32::WHITE,
    );
}

pub(super) fn descriptor() -> ModuleDescriptor {
    ModuleDescriptor {
        id: "devtools.timestamp",
        name: "时间戳转换",
        description: "秒、毫秒与北京时间互转",
        order: 20,
        size: [680.0, 570.0],
        toast_offset: -48.0,
        matches: |input| {
            let _ = input;
            matches!(input, devtools_module_api::Selection::Text(text) if timestamp::matches(text))
        },
        paint_icon,
        create,
        validate_config: |config| config.is_object(),
        default_hotkey: "",
    }
}

impl ToolModule for Tool {
    fn activate(&mut self, input: Option<devtools_module_api::Selection>) -> Vec<ToolEffect> {
        if let Some(Selection::Text(text)) = input {
            self.timestamp.load(&text);
        }
        std::mem::take(&mut self.effects)
    }
    fn render(&mut self, ui: &mut Ui, palette: &style::Palette) -> Vec<ToolEffect> {
        self.palette = *palette;
        self.timestamp_ui(ui);
        std::mem::take(&mut self.effects)
    }
    fn pinned(&self) -> bool {
        self.timestamp.pinned
    }
}

#[cfg(test)]
mod tests;
