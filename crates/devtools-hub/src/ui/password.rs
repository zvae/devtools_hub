//! 随机密码生成器的宿主界面；生成算法在独立的 devtools-password 中。
//! 密码仅保存在当前内存状态，复制走带隐私标记的写入流程，不进入本应用历史。

use devtools_clipboard::{uti, windows_formats, Representation, Snapshot};
use devtools_password::{Options, MAX_LENGTH, MIN_LENGTH};
use egui::{
    Align, Align2, Color32, FontId, Frame, Key, Layout, Margin, Modifiers, Rect, RichText, Sense,
    Stroke, Ui, Vec2,
};

use super::icons::Icon;
use super::widgets::{centered_row, drag_window, icon_button};
use super::{style, HubApp, Page};
use crate::platform::panel::{self as native_panel, Anchor};
use crate::platform::Placement;

#[derive(Default)]
pub(super) struct PasswordView {
    pub pinned: bool,
    pub options: Options,
    pub value: String,
    error: Option<String>,
}

impl PasswordView {
    /// 失败时清除旧结果，避免用户误以为旧密码符合刚修改的配置。
    fn regenerate(&mut self) {
        match devtools_password::generate(self.options) {
            Ok(value) => {
                self.value = value;
                self.error = None;
            }
            Err(error) => {
                self.value.clear();
                self.error = Some(error.to_string());
            }
        }
    }
}

impl HubApp {
    pub(super) fn open_password_generator(&mut self) {
        self.quick.selection = None;
        self.password.regenerate();
        self.set_page(Page::PasswordGenerator);
        native_panel::show(Anchor::Placement(Placement::ScreenCenter), [680.0, 560.0]);
    }

    pub(super) fn password_ui(&mut self, ui: &mut Ui) {
        let palette = self.palette(ui.ctx());
        let mut close = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        // 键盘复制与按钮复制走同一条隐私写入路径，不交给 egui 输出普通文本。
        let mut copy = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::C));
        Frame::NONE
            .inner_margin(Margin::symmetric(style::PAD, 12))
            .show(ui, |ui| {
                drag_window(ui, ui.max_rect());
                centered_row(ui, 28.0, |ui| {
                    #[cfg(target_os = "macos")]
                    {
                        close |= super::widgets::traffic_light_close(ui).clicked();
                        ui.add_space(4.0);
                    }
                    ui.label(
                        RichText::new("密码生成器")
                            .size(16.0)
                            .strong()
                            .color(palette.text),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        #[cfg(not(target_os = "macos"))]
                        {
                            close |= icon_button(ui, Icon::Close, false, &palette).clicked();
                        }
                        if icon_button(ui, Icon::Pin, self.password.pinned, &palette)
                            .on_hover_text("固定窗口")
                            .clicked()
                        {
                            self.password.pinned = !self.password.pinned;
                        }
                    });
                });
            });
        let view = &mut self.password;
        let mut regenerate = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            Frame::NONE
                .inner_margin(Margin::symmetric(24, 14))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 14.0;
                    ui.label(
                        RichText::new("为每个账号生成不同的随机密码")
                            .size(12.0)
                            .color(palette.weak),
                    );
                    Frame::new()
                        .fill(if palette.dark {
                            Color32::from_rgb(32, 43, 61)
                        } else {
                            Color32::from_rgb(240, 245, 255)
                        })
                        .corner_radius(14.0)
                        .inner_margin(18.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 10.0;
                            let (title, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 20.0),
                                Sense::hover(),
                            );
                            ui.painter().text(
                                title.left_center(),
                                Align2::LEFT_CENTER,
                                "新密码",
                                FontId::proportional(11.5),
                                palette.secondary,
                            );
                            ui.painter().text(
                                title.right_center(),
                                Align2::RIGHT_CENTER,
                                format!("{} 字符", view.value.len()),
                                FontId::proportional(11.5),
                                palette.weak,
                            );
                            if let Some(error) = &view.error {
                                ui.label(RichText::new(error).size(13.0).color(palette.danger));
                            } else {
                                // 只通过明确的复制操作写入剪贴板，避免选中文本触发普通复制而绕过隐私标记。
                                ui.add(
                                    egui::Label::new(password_job(
                                        &view.value,
                                        &palette,
                                        ui.available_width(),
                                    ))
                                    .wrap()
                                    .selectable(false),
                                );
                            }
                            let (buttons, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 36.0),
                                Sense::hover(),
                            );
                            let narrow = buttons.width() < 350.0;
                            let width = if narrow {
                                (buttons.width() - 10.0) / 2.0
                            } else {
                                124.0
                            };
                            let refresh = Rect::from_min_size(buttons.min, Vec2::new(width, 36.0));
                            regenerate |= ui
                                .put(
                                    refresh,
                                    egui::Button::new("重新生成")
                                        .fill(palette.card)
                                        .stroke(Stroke::new(1.0, palette.border))
                                        .corner_radius(8.0),
                                )
                                .clicked();
                            let copy_rect = Rect::from_min_size(
                                egui::pos2(buttons.right() - width, buttons.top()),
                                Vec2::new(width, 36.0),
                            );
                            ui.scope(|ui| {
                                if view.value.is_empty() {
                                    ui.disable();
                                }
                                copy |= ui
                                    .put(
                                        copy_rect,
                                        egui::Button::new(
                                            RichText::new("复制密码").color(Color32::WHITE),
                                        )
                                        .fill(palette.accent)
                                        .stroke(Stroke::NONE)
                                        .corner_radius(8.0),
                                    )
                                    .clicked();
                            });
                        });
                    Frame::new()
                        .fill(palette.card)
                        .stroke(Stroke::new(1.0, palette.border))
                        .corner_radius(14.0)
                        .inner_margin(18.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 8.0;
                            ui.label(
                                RichText::new("自定义密码")
                                    .size(13.0)
                                    .strong()
                                    .color(palette.text),
                            );
                            let (row, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 28.0),
                                Sense::hover(),
                            );
                            ui.painter().text(
                                row.left_center(),
                                Align2::LEFT_CENTER,
                                "密码长度",
                                FontId::proportional(12.0),
                                palette.secondary,
                            );
                            let count = Rect::from_min_size(
                                egui::pos2(row.right() - 80.0, row.top()),
                                Vec2::new(80.0, 28.0),
                            );
                            regenerate |= ui
                                .put(
                                    count,
                                    egui::DragValue::new(&mut view.options.length)
                                        .range(MIN_LENGTH..=MAX_LENGTH)
                                        .suffix(" 位")
                                        .speed(1.0),
                                )
                                .changed();
                            ui.spacing_mut().slider_width = ui.available_width();
                            regenerate |= ui
                                .add(
                                    egui::Slider::new(
                                        &mut view.options.length,
                                        MIN_LENGTH..=MAX_LENGTH,
                                    )
                                    .show_value(false),
                                )
                                .changed();
                            ui.separator();
                            regenerate |= option_row(
                                ui,
                                "包含数字",
                                "0–9",
                                &mut view.options.numbers,
                                &palette,
                            );
                            regenerate |= option_row(
                                ui,
                                "包含符号",
                                "! @ # $ % & …",
                                &mut view.options.symbols,
                                &palette,
                            );
                            ui.label(
                                RichText::new("始终包含大写与小写字母")
                                    .size(11.0)
                                    .color(palette.weak),
                            );
                        });
                    ui.label(
                        RichText::new("本机生成 · 不保存密码 · 复制不进入本应用历史")
                            .size(11.0)
                            .color(palette.weak),
                    );
                });
        });
        if regenerate {
            self.password.regenerate();
        }
        if copy && !self.password.value.is_empty() {
            let snapshot = password_snapshot(&self.password.value);
            let pasteboard = self.pasteboard();
            self.shared.write_pasteboard(|| pasteboard.write(&snapshot));
            self.show_toast("已复制密码");
        }
        if close {
            self.hide_panel(true);
        }
    }
}

/// 字母、数字、符号以不同颜色区分，长密码自然换行而不溢出窗口。
fn password_job(value: &str, palette: &style::Palette, width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    // 每个字符均可换行，100 位密码也不会撑宽窗口。
    job.wrap.break_anywhere = true;
    for ch in value.chars() {
        let color = if ch.is_ascii_digit() {
            palette.accent
        } else if ch.is_ascii_punctuation() {
            palette.warn
        } else {
            palette.text
        };
        job.append(
            &ch.to_string(),
            0.0,
            egui::TextFormat {
                font_id: FontId::monospace(if width < 350.0 { 19.0 } else { 23.0 }),
                color,
                line_height: Some(32.0),
                ..Default::default()
            },
        );
    }
    job
}

/// 设置行整体可点击，开关保留键盘和辅助功能的选中状态。
fn option_row(
    ui: &mut Ui,
    label: &str,
    example: &str,
    value: &mut bool,
    palette: &style::Palette,
) -> bool {
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *value, label)
    });
    ui.painter().text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        palette.text,
    );
    let example_width = if rect.width() < 350.0 { 90.0 } else { 150.0 };
    if rect.width() >= 260.0 {
        ui.painter().text(
            egui::pos2(rect.right() - 56.0 - example_width, rect.center().y),
            Align2::LEFT_CENTER,
            example,
            FontId::monospace(11.5),
            palette.weak,
        );
    }
    let switch = Rect::from_center_size(
        rect.right_center() - Vec2::new(19.0, 0.0),
        Vec2::new(38.0, 22.0),
    );
    ui.painter().rect_filled(
        switch,
        11.0,
        if *value {
            palette.accent
        } else {
            palette.track
        },
    );
    let x = if *value {
        switch.right() - 11.0
    } else {
        switch.left() + 11.0
    };
    ui.painter()
        .circle_filled(egui::pos2(x, switch.center().y), 8.0, Color32::WHITE);
    response.changed()
}

/// 同时携带 macOS 与 Windows 的隐私标记；Windows 写回层保留注册格式，macOS 使用 ConcealedType。
fn password_snapshot(value: &str) -> Snapshot {
    let mut snapshot = Snapshot::from_text(value);
    snapshot.items[0].push(Representation::new(uti::CONCEALED, Vec::new()));
    for (name, flag) in [
        (windows_formats::EXCLUDE_FROM_MONITOR, 1u32),
        (windows_formats::CAN_INCLUDE_IN_HISTORY, 0),
        ("CanUploadToCloudClipboard", 0),
    ] {
        snapshot.items[0].push(Representation::new(
            format!("{}{name}", windows_formats::RAW_PREFIX),
            flag.to_le_bytes().to_vec(),
        ));
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_password_has_cross_platform_privacy_markers() {
        let snapshot = password_snapshot("sample-password");
        assert_eq!(
            snapshot.first(uti::PLAIN_TEXT),
            Some(b"sample-password".as_slice())
        );
        assert!(snapshot.has_type(uti::CONCEALED));
        let types = snapshot
            .representations()
            .map(|rep| rep.uti.clone())
            .collect::<Vec<_>>();
        assert!(devtools_clipboard::rules::CaptureRules::default()
            .check_markers(&types, &devtools_clipboard::SourceApp::default())
            .is_err());
        let formats = windows_formats::windows_formats_from_snapshot(&snapshot);
        for (name, value) in [
            (windows_formats::EXCLUDE_FROM_MONITOR, 1u32),
            (windows_formats::CAN_INCLUDE_IN_HISTORY, 0),
            ("CanUploadToCloudClipboard", 0),
        ] {
            assert!(formats.iter().any(|format| format.target
                == windows_formats::FormatTarget::Registered(name.into())
                && format.data == value.to_le_bytes()));
        }
    }
}
