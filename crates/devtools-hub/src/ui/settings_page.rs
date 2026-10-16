//! 设置页（对应 PasteNow 偏好设置中的 General / Shortcuts / Exclusion Rules / Advanced）。
//!
//! 采用 macOS 偏好设置常见的“左侧标签、右侧控件”行布局，布尔项使用开关。

use devtools_clipboard::{ClipList, ExcludedApp, SourceApp, Store, StoreStats, FAVORITES_LIST_ID};
use egui::{Align, Color32, Key, Layout, Response, RichText, Sense, Stroke, Ui, Vec2};

use super::fonts;
use super::icons::{self, Icon};
use super::panel::{centered_row, icon_button, keycaps};
use super::style::{self, Palette};
use super::{HubApp, Page};
use crate::platform::{keyboard, login_item, panel::Placement, workspace, Backdrop};
use crate::settings::{Retention, Settings, POLL_INTERVALS};
use crate::update::{self, Install, Release, Status, REPOSITORY};
use crate::{hotkey, keys, platform};

#[derive(Default)]
pub struct SettingsView {
    recording_hotkey: bool,
    manual_bundle_id: String,
    new_list_name: String,
    renaming: Option<(i64, String)>,
    confirm_clear: bool,
    stats: StoreStats,
    recent_sources: Vec<SourceApp>,
    lists: Vec<ClipList>,
    launch_at_login: bool,
    message: Option<String>,
    /// 下一帧滚动到底部的更新信息（从菜单栏的“发现新版本”打开设置时）。
    pub reveal_update: bool,
}

impl SettingsView {
    pub fn refresh(&mut self, store: &Store) {
        self.stats = store.stats().unwrap_or_default();
        self.recent_sources = store.recent_sources(30).unwrap_or_default();
        self.lists = store.lists().unwrap_or_default();
        self.launch_at_login = login_item::is_enabled();
        self.confirm_clear = false;
        self.message = None;
    }
}

impl HubApp {
    pub(super) fn settings_ui(&mut self, ui: &mut Ui) {
        let palette = self.palette(ui.ctx());
        let escape = !self.settings_view.recording_hotkey
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
        centered_row(ui, 30.0, |ui| {
            if icon_button(ui, Icon::Back, false, &palette).clicked() || escape {
                self.cancel_hotkey_recording();
                self.page = Page::Panel;
                self.panel.reset_for_show();
            }
            ui.label(
                RichText::new("设置")
                    .size(15.0)
                    .family(egui::FontFamily::Name(fonts::SEMIBOLD.into()))
                    .color(palette.text),
            );
        });

        let mut settings = self.settings.clone();
        let mut open_url = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                section(ui, &palette, "通用", |ui| {
                    self.general_section(ui, &palette, &mut settings)
                });
                if self.backdrop != Backdrop::Opaque {
                    section(ui, &palette, "外观", |ui| {
                        appearance_section(ui, &palette, &mut settings)
                    });
                }
                section(ui, &palette, "粘贴", |ui| {
                    self.paste_section(ui, &palette, &mut settings)
                });
                section(ui, &palette, "鼠标中键", |ui| {
                    middle_click_section(ui, &palette, &mut settings)
                });
                section(ui, &palette, "记录", |ui| {
                    record_section(ui, &palette, &mut settings)
                });
                section(ui, &palette, "隐私与排除规则", |ui| {
                    self.rules_section(ui, &palette, &mut settings)
                });
                section(ui, &palette, "列表", |ui| {
                    self.lists_section(ui, &palette)
                });
                section(ui, &palette, "数据", |ui| self.data_section(ui, &palette));
                ui.add_space(10.0);
                // 版本信息在页面底部居中。
                let about = egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(12, 0))
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            open_url = self.about_section(ui, &palette, &mut settings);
                        });
                    });
                if std::mem::take(&mut self.settings_view.reveal_update) {
                    about.response.scroll_to_me(Some(Align::Max));
                }
            });
        if let Some(url) = open_url {
            // 面板浮在其他窗口之上，打开浏览器后收起，免得挡住网页。
            workspace::open_url(&url);
            self.hide_panel(false);
        }

        if settings != self.settings {
            let hotkey_changed = settings.hotkey != self.settings.hotkey;
            self.settings = settings;
            if hotkey_changed {
                self.apply_hotkey();
            }
            self.persist_settings();
            self.sync_middle_click();
        }
    }

    /// 离开设置页时若仍在录制，恢复原快捷键的注册。
    pub(super) fn cancel_hotkey_recording(&mut self) {
        if std::mem::take(&mut self.settings_view.recording_hotkey) {
            self.apply_hotkey();
        }
    }

    fn apply_hotkey(&mut self) {
        self.hotkey_error = match &mut self.hotkeys {
            Some(service) => service.apply(&self.settings.hotkey).err(),
            None => Some("全局快捷键服务不可用".into()),
        };
    }

    fn general_section(&mut self, ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
        if self.settings_view.recording_hotkey {
            row(ui, "唤起快捷键", |ui| {
                ui.label(RichText::new("请按下新的组合键…").color(palette.accent));
            });
            hint(
                ui,
                palette,
                &format!(
                    "需要包含 {} 中的至少一个，{} 取消",
                    keys::MODIFIER_HINT,
                    keys::ESCAPE
                ),
            );
            let pressed = ui.input(|i| {
                i.events.iter().find_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            match pressed {
                Some((Key::Escape, _)) => {
                    self.settings_view.recording_hotkey = false;
                    self.apply_hotkey();
                }
                Some((key, modifiers)) => {
                    if let Some(spec) = hotkey::from_egui(modifiers, key) {
                        settings.hotkey = spec;
                        self.settings_view.recording_hotkey = false;
                        // 录制期间注销了旧快捷键，这里无论是否变化都重新注册。
                        if settings.hotkey == self.settings.hotkey {
                            self.apply_hotkey();
                        }
                    }
                }
                None => {}
            }
        } else {
            row(ui, "唤起快捷键", |ui| {
                if ui.button("录制").clicked() {
                    self.settings_view.recording_hotkey = true;
                    // 避免录制时按下旧快捷键直接把面板关掉。
                    if let Some(service) = &mut self.hotkeys {
                        service.suspend();
                    }
                }
                let parts = hotkey::display_parts(&settings.hotkey);
                let keys: Vec<&str> = parts.iter().map(String::as_str).collect();
                keycaps(ui, &keys, palette);
            });
            let default = Settings::default().hotkey;
            let restore = format!("恢复默认 {}", hotkey::display(&default));
            if settings.hotkey != default && ui.link(RichText::new(restore).size(11.5)).clicked() {
                settings.hotkey = default;
            }
        }
        if let Some(error) = &self.hotkey_error {
            ui.label(RichText::new(error).color(palette.warn).size(11.5));
        }
        divider(ui, palette);
        row(ui, "出现位置", |ui| {
            egui::ComboBox::from_id_salt("placement")
                .width(120.0)
                .selected_text(settings.placement.label())
                .show_ui(ui, |ui| {
                    for placement in Placement::ALL {
                        ui.selectable_value(&mut settings.placement, placement, placement.label());
                    }
                });
        });
        hint(
            ui,
            palette,
            &format!(
                "快捷键唤起时的位置；点击{}时总是在图标旁弹出",
                platform::TRAY_NAME
            ),
        );
        divider(ui, palette);
        switch_row(ui, palette, "失焦时自动隐藏", &mut settings.hide_on_blur);
        divider(ui, palette);
        let mut launch = self.settings_view.launch_at_login;
        if switch_row(ui, palette, "登录时启动", &mut launch) {
            match login_item::set_enabled(launch) {
                Ok(()) => self.settings_view.launch_at_login = launch,
                Err(err) => self.settings_view.message = Some(format!("{err:#}")),
            }
        }
        divider(ui, palette);
        switch_row(
            ui,
            palette,
            "自动检查更新",
            &mut settings.auto_check_updates,
        );
        hint(
            ui,
            palette,
            "启动时和之后每天检查一次 GitHub 上是否有新版本",
        );
    }

    /// 设置页底部：版本号、检查更新与仓库地址。返回要在浏览器中打开的地址。
    fn about_section(
        &self,
        ui: &mut Ui,
        palette: &Palette,
        settings: &mut Settings,
    ) -> Option<String> {
        let mut open = None;
        let state = self.shared.update_state();
        centered_line(ui, "version", 26.0, |ui| {
            hint(
                ui,
                palette,
                &format!("DevTools Hub {}", update::CURRENT_VERSION),
            );
            ui.add_space(4.0);
            if ui
                .add_enabled(!state.checking, egui::Button::new("检查更新"))
                .clicked()
            {
                update::check_now(&self.shared);
            }
            if state.checking {
                ui.add(egui::Spinner::new().size(12.0).color(palette.weak));
            }
        });
        match &state.status {
            Status::Unknown => {}
            Status::UpToDate => {
                let checked = state
                    .checked_at
                    .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|since| {
                        let now = devtools_clipboard::store::now_ms();
                        format!(
                            " · 检查于 {}",
                            style::card_time(since.as_millis() as i64, now)
                        )
                    })
                    .unwrap_or_default();
                hint(ui, palette, &format!("已是最新版本{checked}"));
            }
            Status::Failed(error) => {
                ui.label(
                    RichText::new(format!("检查更新失败：{error}"))
                        .size(11.5)
                        .color(palette.warn),
                );
            }
            Status::Available(release) => {
                ui.label(
                    RichText::new(format!("发现新版本 v{}", release.version))
                        .size(12.5)
                        .color(palette.accent),
                );
                let install = state.install.as_ref();
                self.release_actions(ui, palette, release, install, settings, &mut open);
            }
        }
        let link = RichText::new(format!("GitHub 仓库：{}", repo_label())).size(11.5);
        if ui.link(link).on_hover_text(REPOSITORY).clicked() {
            open = Some(REPOSITORY.to_string());
        }
        open
    }

    /// 新版本的操作：应用内更新（下载进度、取消、失败重试），不支持时用浏览器下载；更新说明、跳过此版本。
    fn release_actions(
        &self,
        ui: &mut Ui,
        palette: &Palette,
        release: &Release,
        install: Option<&Install>,
        settings: &mut Settings,
        open: &mut Option<String>,
    ) {
        match install {
            Some(Install::Downloading { received, total }) => {
                centered_row(ui, 26.0, |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("取消").clicked() {
                            update::cancel_install(&self.shared);
                        }
                        let progress = if *total > 0 {
                            *received as f32 / *total as f32
                        } else {
                            0.0
                        };
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .desired_width(ui.available_width() - 4.0)
                                .desired_height(6.0)
                                .fill(palette.accent),
                        );
                    });
                });
                let size = |bytes: u64| style::byte_size(bytes);
                hint(
                    ui,
                    palette,
                    &format!("正在下载 {} / {}", size(*received), size(*total)),
                );
                return;
            }
            Some(Install::Installing | Install::Installed(_)) => {
                centered_line(ui, "installing", 26.0, |ui| {
                    ui.add(egui::Spinner::new().size(12.0).color(palette.weak));
                    hint(ui, palette, "正在安装，完成后自动重新启动…");
                });
                return;
            }
            Some(Install::Failed(error)) => {
                ui.label(
                    RichText::new(format!("自动更新失败：{error}"))
                        .size(11.5)
                        .color(palette.warn),
                );
            }
            None => {}
        }
        let failed = install.is_some();
        let installable = update::can_install(release);
        let url = release.download_or_page();
        centered_line(ui, "release-actions", 26.0, |ui| {
            if installable {
                let label = if failed { "重试" } else { "立即更新" };
                let button = ui
                    .button(label)
                    .on_hover_text("下载并安装，完成后自动重新启动");
                if button.clicked() {
                    update::install(&self.shared);
                }
                if failed
                    && ui
                        .link(RichText::new("用浏览器下载").size(11.5))
                        .on_hover_text(url)
                        .clicked()
                {
                    *open = Some(url.to_string());
                }
            } else {
                let reason = match (&release.asset, update::install_target()) {
                    (None, _) => "Release 中没有当前平台的安装包，打开 Release 页面".to_string(),
                    (Some(_), Err(reason)) => format!("{reason}，用浏览器下载"),
                    (Some(_), Ok(_)) => url.to_string(),
                };
                if ui.button("下载").on_hover_text(reason).clicked() {
                    *open = Some(url.to_string());
                }
            }
            let notes = ui
                .link(RichText::new("更新说明").size(11.5))
                .on_hover_text(&release.page_url);
            if notes.clicked() {
                *open = Some(release.page_url.clone());
            }
            if settings.skipped_version.as_ref() == Some(&release.version) {
                hint(ui, palette, "已跳过此版本");
            } else if ui.link(RichText::new("跳过此版本").size(11.5)).clicked() {
                settings.skipped_version = Some(release.version.clone());
            }
        });
        // 打包脚本用的是临时签名：系统按签名识别应用，新版本的签名和旧版本不同。
        if installable && platform::PASTE_NEEDS_PERMISSION {
            hint(
                ui,
                palette,
                "更新后可能需要在“辅助功能”中重新勾选本应用（应用使用临时签名）",
            );
        }
    }

    fn paste_section(&mut self, ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
        switch_row(
            ui,
            palette,
            "直接粘贴到当前应用",
            &mut settings.paste_to_active_app,
        );
        hint(
            ui,
            palette,
            &format!(
                "选中条目后切回之前的应用并模拟 {}；关闭则只复制",
                keys::shortcut(&[keys::COMMAND, "V"])
            ),
        );
        // 只有 macOS 需要授权；Windows 模拟按键不需要。
        if platform::PASTE_NEEDS_PERMISSION {
            divider(ui, palette);
            let trusted = keyboard::accessibility_trusted(false);
            self.accessibility_ok = trusted;
            row(ui, "辅助功能权限", |ui| {
                if trusted {
                    ui.label(RichText::new("已授权").color(palette.ok));
                    let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    icons::paint(ui.painter(), icon_rect, Icon::Check, palette.ok);
                } else if ui.button("去授权").clicked() {
                    keyboard::accessibility_trusted(true);
                    keyboard::open_accessibility_settings();
                }
            });
        }
        divider(ui, palette);
        switch_row(
            ui,
            palette,
            "粘贴后移到最前",
            &mut settings.move_to_top_after_paste,
        );
    }

    fn rules_section(&mut self, ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
        let rules = &mut settings.rules;
        switch_row(ui, palette, "忽略密码类内容", &mut rules.ignore_concealed);
        hint(ui, palette, "密码管理器标记为 Concealed 的内容");
        divider(ui, palette);
        switch_row(ui, palette, "忽略临时内容", &mut rules.ignore_transient);
        hint(ui, palette, "标记为 Transient 的内容，写入后很快会被还原");
        divider(ui, palette);
        switch_row(
            ui,
            palette,
            "忽略自动生成的内容",
            &mut rules.ignore_auto_generated,
        );
        divider(ui, palette);
        switch_row(
            ui,
            palette,
            "忽略 iPhone / iPad 内容",
            &mut rules.ignore_remote,
        );
        divider(ui, palette);
        switch_row(ui, palette, "忽略空白文本", &mut rules.ignore_blank_text);
        divider(ui, palette);
        row(ui, "大小上限", |ui| {
            ui.add(
                egui::DragValue::new(&mut rules.max_size_mb)
                    .range(0..=2048)
                    .suffix(" MB"),
            );
        });
        hint(ui, palette, "超过该大小的内容不记录，0 表示不限制");
        divider(ui, palette);

        ui.label(RichText::new("不记录以下应用中的复制").color(palette.text));
        ui.add_space(2.0);
        let mut remove = None;
        for (index, app) in rules.excluded_apps.iter().enumerate() {
            centered_row(ui, 28.0, |ui| {
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                if let Some(texture) = self.app_icons.get(ui.ctx(), &app.bundle_id) {
                    egui::Image::from_texture(&texture).paint_at(ui, icon_rect);
                }
                let name = if app.name.is_empty() {
                    &app.bundle_id
                } else {
                    &app.name
                };
                ui.label(RichText::new(name).color(palette.text))
                    .on_hover_text(&app.bundle_id);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, Icon::Close, false, palette)
                        .on_hover_text("移除")
                        .clicked()
                    {
                        remove = Some(index);
                    }
                });
            });
        }
        if let Some(index) = remove {
            rules.excluded_apps.remove(index);
        }
        let candidates: Vec<&SourceApp> =
            self.settings_view
                .recent_sources
                .iter()
                .filter(|source| {
                    source.bundle_id.as_deref().is_some_and(|id| {
                        !rules.excluded_apps.iter().any(|app| app.bundle_id == id)
                    })
                })
                .collect();
        ui.add_space(4.0);
        egui::ComboBox::from_id_salt("exclude-recent")
            .selected_text("从最近的来源应用中添加…")
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for source in candidates {
                    if ui.selectable_label(false, source.display_name()).clicked() {
                        rules.excluded_apps.push(ExcludedApp {
                            bundle_id: source.bundle_id.clone().unwrap_or_default(),
                            name: source.name.clone().unwrap_or_default(),
                        });
                    }
                }
            });
        centered_row(ui, 30.0, |ui| {
            let id = self.settings_view.manual_bundle_id.trim().to_string();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let add = ui.add_enabled(!id.is_empty(), egui::Button::new("添加"));
                if add.clicked() && !rules.excluded_apps.iter().any(|app| app.bundle_id == id) {
                    rules.excluded_apps.push(ExcludedApp {
                        bundle_id: id,
                        name: String::new(),
                    });
                    self.settings_view.manual_bundle_id.clear();
                }
                ui.add(
                    style::text_field(&mut self.settings_view.manual_bundle_id)
                        .hint_text(if cfg!(windows) {
                            "或输入程序名，如 KeePass.exe"
                        } else {
                            "或输入 Bundle ID"
                        })
                        .desired_width(ui.available_width()),
                );
            });
        });
    }

    fn lists_section(&mut self, ui: &mut Ui, palette: &Palette) {
        hint(
            ui,
            palette,
            "列表中的条目不会被自动清理，也不会被“清空历史”删除",
        );
        let mut changed = false;
        for list in self.settings_view.lists.clone() {
            divider(ui, palette);
            centered_row(ui, 28.0, |ui| match &mut self.settings_view.renaming {
                Some((id, name)) if *id == list.id => {
                    ui.add(style::text_field(name).desired_width(ui.available_width() - 60.0));
                    if ui.button("保存").clicked() {
                        if let Err(err) = self.store.rename_list(list.id, name) {
                            self.settings_view.message = Some(format!("{err:#}"));
                        }
                        self.settings_view.renaming = None;
                        changed = true;
                    }
                }
                _ => {
                    if list.id == FAVORITES_LIST_ID {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                        icons::paint(ui.painter(), icon_rect, Icon::StarFilled, palette.weak);
                    }
                    ui.label(RichText::new(&list.name).color(palette.text));
                    ui.label(
                        RichText::new(format!("{} 条", list.item_count))
                            .color(palette.weak)
                            .size(11.5),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if list.id != FAVORITES_LIST_ID
                            && icon_button(ui, Icon::Close, false, palette)
                                .on_hover_text("删除列表（条目保留在历史中）")
                                .clicked()
                        {
                            if let Err(err) = self.store.delete_list(list.id) {
                                self.settings_view.message = Some(format!("{err:#}"));
                            }
                            changed = true;
                        }
                        if icon_button(ui, Icon::Pencil, false, palette)
                            .on_hover_text("重命名")
                            .clicked()
                        {
                            self.settings_view.renaming = Some((list.id, list.name.clone()));
                        }
                    });
                }
            });
        }
        divider(ui, palette);
        centered_row(ui, 30.0, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("新建").clicked() {
                    match self.store.create_list(&self.settings_view.new_list_name) {
                        Ok(_) => self.settings_view.new_list_name.clear(),
                        Err(err) => self.settings_view.message = Some(format!("{err:#}")),
                    }
                    changed = true;
                }
                ui.add(
                    style::text_field(&mut self.settings_view.new_list_name)
                        .hint_text("新列表名称")
                        .desired_width(ui.available_width()),
                );
            });
        });
        if changed {
            self.settings_view.lists = self.store.lists().unwrap_or_default();
            self.panel.needs_reload = true;
        }
    }

    fn data_section(&mut self, ui: &mut Ui, palette: &Palette) {
        let stats = self.settings_view.stats;
        row(ui, "记录条数", |ui| {
            ui.label(RichText::new(style::group_digits(stats.item_count)).color(palette.text));
        });
        divider(ui, palette);
        row(ui, "占用空间", |ui| {
            let total = stats.database_bytes + stats.external_bytes;
            ui.label(RichText::new(style::byte_size(total)).color(palette.text));
        });
        hint(ui, palette, &self.data_dir.display().to_string());
        divider(ui, palette);
        centered_row(ui, 30.0, |ui| {
            if ui.button("打开数据目录").clicked() {
                workspace::open_path(&self.data_dir);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.settings_view.confirm_clear {
                    if ui.button("取消").clicked() {
                        self.settings_view.confirm_clear = false;
                    }
                    let confirm =
                        egui::Button::new(RichText::new("确定清空").color(Color32::WHITE))
                            .fill(Color32::from_rgb(0xe5, 0x48, 0x4d));
                    if ui.add(confirm).clicked() {
                        match self.store.clear_history() {
                            Ok(removed) => {
                                self.settings_view.message =
                                    Some(format!("已删除 {removed} 条记录"))
                            }
                            Err(err) => self.settings_view.message = Some(format!("{err:#}")),
                        }
                        self.settings_view.confirm_clear = false;
                        self.settings_view.stats = self.store.stats().unwrap_or_default();
                        self.panel.needs_reload = true;
                    }
                } else if ui.button("清空历史…").clicked() {
                    self.settings_view.confirm_clear = true;
                }
            });
        });
        if self.settings_view.confirm_clear {
            hint(ui, palette, "清空后无法恢复；列表中的条目会保留");
        }
        if let Some(message) = &self.settings_view.message {
            ui.label(RichText::new(message).color(palette.weak).size(11.5));
        }
    }
}

fn middle_click_section(ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
    switch_row(
        ui,
        palette,
        "按下鼠标中键弹出快捷面板",
        &mut settings.middle_click,
    );
    hint(
        ui,
        palette,
        &format!(
            "在其他应用中选中文字后按鼠标中键，会模拟 {} 读取选中的内容（读完恢复剪贴板），\
             列出能处理它的功能，例如选中 JSON 时可以用 JSON 编辑器格式化；没有选中时列出常用功能",
            keys::shortcut(&[keys::COMMAND, "C"])
        ),
    );
    if platform::PASTE_NEEDS_PERMISSION && settings.middle_click {
        if keyboard::accessibility_trusted(false) {
            hint(ui, palette, "开启后，中键点击不会再传给前台应用");
        } else {
            ui.label(
                RichText::new("需要“辅助功能”权限才能监听鼠标中键和读取选中的内容")
                    .color(palette.warn)
                    .size(11.5),
            );
        }
    }
}

fn record_section(ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
    switch_row(ui, palette, "暂停记录", &mut settings.paused);
    divider(ui, palette);
    row(ui, "检查间隔", |ui| {
        let current = POLL_INTERVALS
            .iter()
            .find(|(ms, _)| *ms == settings.poll_interval_ms)
            .map_or_else(
                || format!("{} ms", settings.poll_interval_ms),
                |(_, label)| label.to_string(),
            );
        egui::ComboBox::from_id_salt("poll")
            .width(140.0)
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (ms, label) in POLL_INTERVALS {
                    ui.selectable_value(&mut settings.poll_interval_ms, ms, label);
                }
            });
    });
    divider(ui, palette);
    row(ui, "保留时长", |ui| {
        egui::ComboBox::from_id_salt("retention")
            .width(140.0)
            .selected_text(settings.retention.label())
            .show_ui(ui, |ui| {
                for retention in Retention::ALL {
                    ui.selectable_value(&mut settings.retention, retention, retention.label());
                }
            });
    });
    divider(ui, palette);
    row(ui, "最多保留", |ui| {
        ui.add(
            egui::DragValue::new(&mut settings.max_items)
                .range(0..=1_000_000)
                .suffix(" 条"),
        );
    });
    hint(ui, palette, "0 表示不限制；列表中的条目不计入");
}

fn appearance_section(ui: &mut Ui, palette: &Palette, settings: &mut Settings) {
    row(ui, "背景透明度", |ui| {
        // 行是从右到左排的：先放数值（固定宽度，拖动时滑块不跟着挪），再放滑块。
        let value = RichText::new(format!("{}%", settings.panel_transparency)).color(palette.weak);
        ui.add_sized([34.0, 20.0], egui::Label::new(value));
        ui.spacing_mut().slider_width = 150.0;
        // 滑块会把圆角加到填充条的长度上，全局的胶囊圆角（255）会让填充条越界，换成轨道高度的一半。
        let rail_radius = (ui.spacing().slider_rail_height / 2.0) as u8;
        ui.visuals_mut().widgets.inactive.corner_radius = rail_radius.into();
        ui.add(
            egui::Slider::new(&mut settings.panel_transparency, 0..=100)
                .show_value(false)
                .trailing_fill(true),
        );
    });
    hint(
        ui,
        palette,
        "面板底色的透明度：越大越能透出背后的玻璃效果，越小文字越清晰",
    );
}

/// 仓库地址去掉协议前缀，显示得短一些。
fn repo_label() -> &'static str {
    REPOSITORY.trim_start_matches("https://")
}

/// 分组标题 + 圆角卡片。
fn section(ui: &mut Ui, palette: &Palette, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    ui.add_space(12.0);
    // 标题与卡片内的文字左对齐（卡片内边距 12），和系统设置的分组样式一致。
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 12,
            ..Default::default()
        })
        .show(ui, |ui| {
            ui.label(RichText::new(title).size(12.0).color(palette.weak));
        });
    ui.add_space(2.0);
    style::continuous_frame(
        ui,
        style::CARD_RADIUS,
        palette.card,
        Stroke::new(1.0, palette.card_stroke),
        egui::Margin::symmetric(12, 8),
        |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            add_contents(ui);
        },
    );
}

/// 左侧标签、右侧控件的一行。
fn row(ui: &mut Ui, label: &str, add_control: impl FnOnce(&mut Ui)) {
    centered_row(ui, 30.0, |ui| {
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), add_control);
    });
}

/// 水平居中的一行。egui 的水平布局不能把一组控件整体居中：按上一帧量出的内容宽度在左边留白，
/// 宽度变了就丢弃这一帧重新布局，不会先偏一下再居中。
fn centered_line(ui: &mut Ui, id_salt: &str, height: f32, add_contents: impl FnOnce(&mut Ui)) {
    let id = ui.id().with(id_salt);
    let available = ui.available_width();
    let width = ui.ctx().data(|data| data.get_temp::<f32>(id));
    centered_row(ui, height, |ui| {
        ui.add_space(((available - width.unwrap_or(available)) / 2.0).max(0.0));
        let start = ui.cursor().left();
        add_contents(ui);
        let measured = ui.min_rect().right() - start;
        if width.is_none_or(|width| (width - measured).abs() > 0.5) {
            ui.ctx().data_mut(|data| data.insert_temp(id, measured));
            ui.ctx().request_discard("居中的一行宽度变了");
        }
    });
}

/// 带开关的一行，返回值是否被切换。
fn switch_row(ui: &mut Ui, palette: &Palette, label: &str, value: &mut bool) -> bool {
    let mut changed = false;
    row(ui, label, |ui| {
        changed = toggle(ui, value, palette).changed()
    });
    changed
}

fn hint(ui: &mut Ui, palette: &Palette, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(palette.weak));
}

fn divider(ui: &mut Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, palette.card_stroke),
    );
}

/// iOS / macOS 风格的开关。
fn toggle(ui: &mut Ui, on: &mut bool, palette: &Palette) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(36.0, 20.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let track = palette.track.lerp_to_gamma(palette.accent, t);
    ui.painter().rect_filled(rect, rect.height() / 2.0, track);
    let radius = rect.height() / 2.0 - 2.0;
    let x = egui::lerp(
        (rect.left() + radius + 2.0)..=(rect.right() - radius - 2.0),
        t,
    );
    ui.painter().circle(
        egui::pos2(x, rect.center().y),
        radius,
        Color32::WHITE,
        Stroke::new(0.5, Color32::from_black_alpha(30)),
    );
    response
}
