//! 设置页底部的版本信息：版本号、检查更新、新版本的下载与安装、仓库地址。

use egui::{Align, Layout, RichText, Ui};

use super::rows::{centered_line, hint, hint_text};
use crate::settings::Settings;
use crate::ui::style::{self, Palette};
use crate::ui::widgets::centered_row;
use crate::ui::HubApp;
use crate::update::{self, Install, Release, Status, REPOSITORY};

impl HubApp {
    /// 设置页底部：版本号、检查更新与仓库地址。返回要在浏览器中打开的地址。
    pub(super) fn about_section(
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
                ui.label(hint_text(format!("检查更新失败：{error}"), palette.warn));
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
    pub(super) fn release_actions(
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
                ui.label(hint_text(format!("自动更新失败：{error}"), palette.warn));
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
        // 临时签名时系统按代码哈希识别应用，每个版本都不同；用证书签名时权限保留。
        if installable && update::update_resets_permission() {
            hint(
                ui,
                palette,
                "当前版本使用临时签名：更新后需要在“辅助功能”中删掉本应用再重新添加",
            );
        }
    }
}

/// 仓库地址去掉协议前缀，显示得短一些。
fn repo_label() -> &'static str {
    REPOSITORY.trim_start_matches("https://")
}
