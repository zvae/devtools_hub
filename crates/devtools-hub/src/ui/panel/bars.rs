//! 浮在列表上下的两个玻璃栏：顶部栏（固定按钮、搜索框、设置按钮、分类标签、提示横幅）
//! 与底部栏（暂停按钮、条目数、快捷键说明）。

use egui::{
    Align, Align2, Color32, FontId, Frame, Id, Key, Layout, Margin, PopupCloseBehavior, Rect,
    Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use super::Action;
use crate::platform::panel as native_panel;
use crate::ui::glass;
use crate::ui::icons::{self, Icon};
use crate::ui::menu;
use crate::ui::style::{self, Palette};
use crate::ui::widgets::{centered_row, drag_window, icon_button, keycaps};
use crate::ui::HubApp;
use crate::update::{self, Install, Release};
use crate::{hotkey, keys};

impl HubApp {
    /// 浮在列表上方的顶部栏：搜索框、分类标签与提示横幅，背景是玻璃（见 `glass`）。
    pub(super) fn glass_header(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        window: Rect,
        list: Option<&glass::Source>,
        actions: &mut Vec<Action>,
    ) {
        egui::Area::new(ui.id().with("glass-header"))
            .order(egui::Order::Middle)
            .fixed_pos(window.min)
            .constrain(false)
            .show(ui.ctx(), |ui| {
                ui.set_width(window.width());
                ui.spacing_mut().item_spacing.y = 0.0;
                let bar = glass::Bar::begin(ui.painter());
                let last = Rect::from_min_size(
                    window.min,
                    Vec2::new(window.width(), self.panel.header_height),
                );
                drag_window(ui, last);
                Frame::NONE
                    .inner_margin(Margin {
                        left: style::PAD,
                        right: style::PAD,
                        top: style::PAD,
                        bottom: 4,
                    })
                    .show(ui, |ui| self.header(ui, palette, actions));
                Frame::NONE
                    .inner_margin(Margin {
                        left: style::PAD,
                        right: style::PAD,
                        top: 4,
                        bottom: 8,
                    })
                    .show(ui, |ui| self.chips(ui, palette, actions));
                if let Some(error) = self.panel.error.clone() {
                    Frame::NONE
                        .inner_margin(Margin::symmetric(style::PAD, 4))
                        .show(ui, |ui| {
                            ui.label(RichText::new(error).color(palette.warn));
                        });
                }
                if self.settings.paste_to_active_app && !self.accessibility_ok {
                    accessibility_banner(ui, palette, actions);
                }
                if let Some(release) = self
                    .pending_update()
                    .filter(|release| self.update_dismissed.as_ref() != Some(&release.version))
                {
                    let install = self.shared.update_state().install;
                    update_banner(ui, palette, &release, install.as_ref(), actions);
                }

                let height = ui.min_rect().bottom() - window.top();
                let rect = Rect::from_min_size(window.min, Vec2::new(window.width(), height));
                let liquid = self.liquid.as_ref().map(|liquid| (liquid, 0));
                let tint = glass::tint(palette, liquid.is_some());
                bar.finish(
                    ui.painter(),
                    rect,
                    glass::Edge::Top,
                    native_panel::CORNER_RADIUS as f32,
                    list,
                    tint,
                    liquid,
                );
                if (height - self.panel.header_height).abs() > 0.5 {
                    self.panel.header_height = height;
                    ui.ctx().request_discard("顶部栏的高度变了");
                }
            });
    }

    /// 浮在列表下方的底部栏：暂停按钮、条目数与快捷键说明，背景与顶部栏一样是玻璃。
    pub(super) fn glass_footer(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        window: Rect,
        list: Option<&glass::Source>,
        actions: &mut Vec<Action>,
    ) {
        egui::Area::new(ui.id().with("glass-footer"))
            .order(egui::Order::Middle)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(window.left_bottom())
            .constrain(false)
            .show(ui.ctx(), |ui| {
                ui.set_width(window.width());
                let bar = glass::Bar::begin(ui.painter());
                let last = Rect::from_min_max(
                    window.left_bottom() - Vec2::new(0.0, self.panel.footer_height),
                    window.right_bottom(),
                );
                drag_window(ui, last);
                Frame::NONE
                    .inner_margin(Margin {
                        left: style::PAD,
                        right: style::PAD,
                        top: 4,
                        bottom: 8,
                    })
                    .show(ui, |ui| self.footer(ui, palette, actions));
                let height = ui.min_rect().height();
                let rect = Rect::from_min_max(
                    window.left_bottom() - Vec2::new(0.0, height),
                    window.right_bottom(),
                );
                let liquid = self.liquid.as_ref().map(|liquid| (liquid, 1));
                let tint = glass::tint(palette, liquid.is_some());
                bar.finish(
                    ui.painter(),
                    rect,
                    glass::Edge::Bottom,
                    native_panel::CORNER_RADIUS as f32,
                    list,
                    tint,
                    liquid,
                );
                if (height - self.panel.footer_height).abs() > 0.5 {
                    self.panel.footer_height = height;
                    ui.ctx().request_discard("底部栏的高度变了");
                }
            });
    }

    /// 顶部第一行：固定窗口按钮、搜索框、设置按钮。
    fn header(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<Action>) {
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
                .on_hover_text(format!("设置  {}", keys::shortcut(&[keys::COMMAND, ","])))
                .clicked()
            {
                actions.push(Action::OpenSettings);
            }
        });
    }

    /// 搜索框：胶囊形外框加放大镜图标，里面是无边框的单行输入框。
    /// 面板可见且没有菜单打开时，焦点总是留在搜索框里，直接打字就是搜索。
    fn search_field(&mut self, ui: &mut Ui, width: f32, palette: &Palette) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::hover());
        let id = Id::new("search");
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
            StrokeKind::Inside,
        );
        let icon_rect = Rect::from_center_size(
            egui::pos2(rect.left() + 17.0, rect.center().y),
            Vec2::splat(14.0),
        );
        icons::paint(ui.painter(), icon_rect, Icon::Search, palette.weak);
        let edit_rect = Rect::from_min_max(
            egui::pos2(rect.left() + 31.0, rect.top()),
            egui::pos2(rect.right() - 14.0, rect.bottom()),
        );
        // 用不推进布局光标的子 Ui 放输入框：`ui.put` 会把光标移到输入框右边缘（比外框靠左），
        // 右侧的设置按钮就会比左侧的固定按钮贴得更近。
        let mut edit_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(edit_rect)
                .layout(Layout::centered_and_justified(egui::Direction::TopDown)),
        );
        let response = edit_ui.add(
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
        let wants_focus = self.panel.focus_search
            || (!response.has_focus() && !egui::Popup::is_any_open(ui.ctx()));
        if wants_focus && (native_panel::is_visible() || cfg!(test)) {
            response.request_focus();
            self.panel.focus_search = false;
        }
    }

    /// 分类标签一行（放不下时横向滚动）与右侧的“新建列表”按钮。
    fn chips(&mut self, ui: &mut Ui, palette: &Palette, actions: &mut Vec<Action>) {
        let filters = self.panel.filters(&self.settings);
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
                            let response = chip(ui, &label, selected, palette);
                            if response.clicked() {
                                self.panel.set_filter(filter);
                            }
                            if response.secondary_clicked() {
                                actions.push(Action::TabMenu(filter.tab()));
                            }
                        }
                    });
                });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let plus = icon_button(ui, Icon::Plus, false, palette).on_hover_text("新建列表");
                if std::mem::take(&mut self.panel.open_new_list) {
                    egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&plus));
                }
                let shown = egui::Popup::menu(&plus)
                    // 点击输入框不关闭。
                    .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        menu::begin(ui, 220.0);
                        let heading = if self.panel.new_list_item.is_some() {
                            "新建列表并加入当前条目"
                        } else {
                            "新建列表"
                        };
                        menu::heading(ui, palette, heading);
                        if new_list_row(ui, &mut self.panel.new_list_name, palette, true) {
                            actions.push(Action::CreateList {
                                name: std::mem::take(&mut self.panel.new_list_name),
                                add_item: self.panel.new_list_item.take(),
                            });
                            ui.close();
                        }
                        menu::separator(ui, palette);
                        if menu::item(ui, palette, "编辑分类标签…").clicked() {
                            actions.push(Action::EditTabs);
                            ui.close();
                        }
                    });
                if shown.is_none() {
                    self.panel.new_list_item = None;
                }
            });
        });
    }

    /// 底部栏的内容：暂停按钮、条目总数与快捷键说明按钮。
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

    /// 悬停键盘按钮时显示的快捷键说明。
    fn shortcut_help(&self, ui: &mut Ui, palette: &Palette) {
        let target = native_panel::previous_app_name()
            .map_or_else(|| "粘贴".to_string(), |app| format!("粘贴到 {app}"));
        let paste = if self.settings.paste_to_active_app {
            format!("{target}（或双击）")
        } else {
            "复制（或双击）".to_string()
        };
        let hotkey = hotkey::display_parts(&self.settings.hotkey);
        let mut rows: Vec<(Vec<&str>, &str)> = vec![
            (vec![keys::RETURN], paste.as_str()),
            (vec![keys::SHIFT, keys::RETURN], "以纯文本粘贴"),
            (vec![keys::COMMAND, keys::RETURN], "仅复制"),
            (vec![keys::COMMAND, "1–9"], "快速粘贴第 N 条"),
            (vec![keys::SPACE], "预览（搜索框为空时）"),
            (vec![keys::COMMAND, "D"], "收藏 / 取消收藏"),
            (vec![keys::COMMAND, keys::DELETE], "删除"),
            (vec![keys::TAB], "切换分类"),
            (vec![keys::COMMAND, ","], "设置"),
            (vec![keys::ESCAPE], "清空搜索 / 关闭"),
        ];
        // 设置里删掉快捷键后按键列表是空的：直接说明，免得只看到一句“在任意应用中唤起”。
        if hotkey.is_empty() {
            rows.push((Vec::new(), "未设置唤起快捷键"));
        } else {
            rows.push((
                hotkey.iter().map(String::as_str).collect(),
                "在任意应用中唤起",
            ));
        }
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
}

/// 新建列表的输入框和按钮（菜单里用），回车或点“新建”提交，返回是否提交。
fn new_list_row(ui: &mut Ui, name: &mut String, palette: &Palette, autofocus: bool) -> bool {
    Frame::NONE
        .inner_margin(Margin::symmetric(4, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let edit = ui.add(
                    style::text_field(name)
                        .hint_text("新列表名称")
                        .background_color(palette.field)
                        .desired_width(146.0),
                );
                // 先判断回车：回车会让输入框失去焦点，之后才重新抢回焦点。
                let submit = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if autofocus {
                    edit.request_focus();
                }
                let valid = !name.trim().is_empty();
                let create = egui::Button::new(RichText::new("新建").color(Color32::WHITE))
                    .fill(palette.accent)
                    .min_size(Vec2::new(48.0, 24.0));
                (ui.add_enabled(valid, create).clicked() || submit) && valid
            })
            .inner
        })
        .inner
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

/// 已开启“直接粘贴”但没有辅助功能权限时的提示：只能复制，不能模拟 ⌘V。
fn accessibility_banner(ui: &mut Ui, palette: &Palette, actions: &mut Vec<Action>) {
    let margin = Margin {
        left: style::PAD,
        right: style::PAD,
        top: 0,
        bottom: 8,
    };
    Frame::NONE.outer_margin(margin).show(ui, |ui| {
        style::continuous_frame(
            ui,
            style::CARD_RADIUS,
            palette.warn.gamma_multiply(0.14),
            Stroke::NONE,
            Margin::symmetric(12, 6),
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
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
                // 系统设置里的授权记录属于旧版本（签名不同）时，开关显示已打开但不生效。
                ui.horizontal(|ui| {
                    ui.add_space(14.0 + ui.spacing().item_spacing.x);
                    let text = RichText::new("已打开仍提示？在系统设置中删掉本应用再重新添加")
                        .color(palette.warn.gamma_multiply(0.8))
                        .size(11.0);
                    ui.label(text);
                });
            },
        );
    });
}

/// 发现新版本的提示：更新（应用内安装，进度在设置页底部显示）或下载，也可以关掉（本次运行不再在面板上提示）。
fn update_banner(
    ui: &mut Ui,
    palette: &Palette,
    release: &Release,
    install: Option<&Install>,
    actions: &mut Vec<Action>,
) {
    let version = &release.version;
    let (text, link, action) = match install {
        Some(Install::Downloading { received, total }) => {
            let percent = (*received * 100).checked_div(*total).unwrap_or(0);
            let text = format!("正在下载 v{version} · {percent}%");
            (text, "查看", Action::ShowUpdate)
        }
        Some(Install::Installing | Install::Installed(_)) => {
            (format!("正在安装 v{version}…"), "查看", Action::ShowUpdate)
        }
        Some(Install::Failed(_)) => (
            format!("v{version} 自动更新失败"),
            "查看",
            Action::ShowUpdate,
        ),
        None if update::can_install(release) => (
            format!("发现新版本 v{version}"),
            "更新",
            Action::InstallUpdate,
        ),
        None => {
            let url = release.download_or_page().to_string();
            (
                format!("发现新版本 v{version}"),
                "下载",
                Action::OpenUrl(url),
            )
        }
    };
    let margin = Margin {
        left: style::PAD,
        right: style::PAD,
        top: 0,
        bottom: 8,
    };
    Frame::NONE.outer_margin(margin).show(ui, |ui| {
        style::continuous_frame(
            ui,
            style::CARD_RADIUS,
            palette.accent.gamma_multiply(0.14),
            Stroke::NONE,
            Margin::symmetric(12, 6),
            |ui| {
                centered_row(ui, 20.0, |ui| {
                    ui.label(RichText::new(text).color(palette.accent).size(12.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (rect, close) =
                            ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
                        let color = if close.hovered() {
                            palette.text
                        } else {
                            palette.weak
                        };
                        icons::paint_centered(ui.painter(), rect, 10.0, Icon::Close, color);
                        if close.on_hover_text("不再在面板上提示").clicked() {
                            actions.push(Action::DismissUpdate(release.version.clone()));
                        }
                        if ui.link(RichText::new(link).size(12.0)).clicked() {
                            actions.push(action);
                        }
                    });
                });
            },
        );
    });
}
