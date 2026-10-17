//! 空格 / ⌘Y 打开的大预览：完整内容、元信息与粘贴按钮。

use devtools_clipboard::{ClipItem, ContentKind, FAVORITES_LIST_ID};
use egui::{Align, Color32, Frame, Layout, Margin, RichText, Sense, Stroke, Ui, Vec2};

use super::cards::fit;
use super::Action;
use crate::ui::code_text;
use crate::ui::fonts;
use crate::ui::icons::{self, Icon};
use crate::ui::style::{self, kind_color, kind_icon, Palette};
use crate::ui::widgets::{centered_row, icon_button};
use crate::ui::{HubApp, PasteMode};
use crate::{keys, platform};

/// 预览区最多显示的字符数，超长文本在 egui 里排版会明显变慢。
const PREVIEW_MAX_CHARS: usize = 20_000;

/// 预览中显示的文本（从数据库读出、按 `PREVIEW_MAX_CHARS` 截断），切换条目时重新读取。
pub(super) struct Preview {
    /// 对应的条目 id。
    id: i64,
    text: String,
    /// 是否被截断（只显示了前 `PREVIEW_MAX_CHARS` 个字符）。
    truncated: bool,
    /// 截断前的字符数。
    char_count: usize,
}

impl HubApp {
    /// 选中条目的大预览：顶部是类型与收藏按钮，底部是元信息与粘贴按钮，中间是内容。
    pub(super) fn preview_ui(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<Action>) {
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
            .inner_margin(Margin::symmetric(style::PAD, 0))
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
                        let star = icon_button(ui, icon, favorite, palette).on_hover_text(format!(
                            "收藏  {}",
                            keys::shortcut(&[keys::COMMAND, "D"])
                        ));
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
                            // 按钮文字跟随所在布局的水平对齐，放进居中布局文字才居中。
                            let paste =
                                egui::Button::new(RichText::new("粘贴").color(Color32::WHITE))
                                    .fill(palette.accent)
                                    .min_size(Vec2::new(columns[0].available_width(), 28.0));
                            let paste = columns[0].vertical_centered(|ui| ui.add(paste)).inner;
                            if paste.on_hover_text(keys::RETURN).clicked() {
                                actions.push(Action::Paste(item.id, PasteMode::Original));
                            }
                            let plain = egui::Button::new("以纯文本粘贴")
                                .min_size(Vec2::new(columns[1].available_width(), 28.0));
                            let plain_keys = keys::shortcut(&[keys::SHIFT, keys::RETURN]);
                            let plain = columns[1].vertical_centered(|ui| ui.add(plain)).inner;
                            if plain.on_hover_text(plain_keys).clicked() {
                                actions.push(Action::Paste(item.id, PasteMode::PlainText));
                            }
                        });
                    });

                style::continuous_frame(
                    ui,
                    style::CARD_RADIUS,
                    palette.card,
                    Stroke::new(1.0, palette.card_stroke),
                    Margin::same(10),
                    |ui| {
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
                                        let text: egui::WidgetText =
                                            match self.code.get(&self.store, &item) {
                                                Some(runs) => {
                                                    code_text::preview_job(&runs, palette, 12.5)
                                                        .into()
                                                }
                                                None => RichText::new(&preview.text)
                                                    .monospace()
                                                    .size(12.5)
                                                    .color(palette.text)
                                                    .into(),
                                            };
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
                    },
                );
            });
        self.panel.preview = Some(preview);
    }

    /// 图片预览：等比缩放到可用空间内居中显示。
    fn image_preview(&mut self, ui: &mut Ui, item: &ClipItem) {
        match self.textures.get(ui.ctx(), &self.store, item.id) {
            Some(texture) => {
                let size = fit(texture.size_vec2(), ui.available_size());
                ui.centered_and_justified(|ui| {
                    ui.add(
                        egui::Image::from_texture(&texture)
                            .fit_to_exact_size(size)
                            .corner_radius(style::SMALL_RADIUS),
                    );
                });
            }
            None => {
                ui.label(RichText::new("无法生成该图片格式的预览，粘贴不受影响").weak());
            }
        }
    }
}

/// 文件预览：逐行列出路径，已不存在的文件标出警告，点击在文件管理器中显示。
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
                        &format!("点击在{}中显示", platform::FILE_MANAGER)
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
