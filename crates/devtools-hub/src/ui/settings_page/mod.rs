//! 设置页（对应 PasteNow 偏好设置中的 General / Shortcuts / Exclusion Rules / Advanced）。
//!
//! 采用 macOS 偏好设置常见的“左侧标签、右侧控件”行布局，布尔项使用开关。
//!
//! 各分组分在不同文件里：
//! - `general`：通用、外观、粘贴、鼠标中键、截屏与贴图、记录；
//! - `rules`：隐私与排除规则；
//! - `lists`：分类标签、列表与数据；
//! - `about`：页面底部的版本信息与检查更新；
//! - `rows`：分组卡片、行、开关等通用的行控件。

mod about;
mod general;
mod lists;
mod rows;
mod rules;

use devtools_clipboard::{ClipList, SourceApp, Store, StoreStats};
use egui::{Align, Key, Rect, RichText, Ui, Vec2};

use super::fonts;
use super::glass;
use super::icons::Icon;
use super::style::{self, Palette};
use super::widgets::{centered_row, drag_window, icon_button};
use super::{HubApp, Page};
use crate::hotkey::HotkeyAction;
use crate::platform::{login_item, workspace, Backdrop};
use general::{appearance_section, middle_click_section, record_section};
use rows::section;

/// 设置页自己的界面状态（设置本身保存在 `HubApp::settings`）。
#[derive(Default)]
pub struct SettingsView {
    /// 正在录制的全局快捷键（录制期间旧快捷键暂时注销）。
    recording_hotkey: Option<HotkeyAction>,
    /// 排除规则里手动输入的 Bundle ID / 程序名。
    manual_bundle_id: String,
    /// “列表”分组里输入的新列表名。
    new_list_name: String,
    /// 正在重命名的列表：（列表 id，输入框里的名字）。
    renaming: Option<(i64, String)>,
    /// 点了“清空历史…”，等待再确认一次。
    confirm_clear: bool,
    // 以下在打开设置页时从数据库和系统读取一次（见 `refresh`）。
    stats: StoreStats,
    recent_sources: Vec<SourceApp>,
    lists: Vec<ClipList>,
    launch_at_login: bool,
    /// 操作结果或错误，显示在“数据”分组底部。
    message: Option<String>,
    /// 下一帧滚动到底部的更新信息（从菜单栏的“发现新版本”打开设置时）。
    pub reveal_update: bool,
    /// 下一帧滚动到“分类标签”分组（从面板的“编辑分类标签…”打开设置时）。
    pub reveal_tabs: bool,
    /// 下一帧滚动到“截屏与贴图”分组（没有屏幕录制权限、无法截屏时）。
    pub reveal_snip: bool,
    /// 浮在内容上面的标题栏的高度（上一帧量出来的）。
    header_height: f32,
}

impl SettingsView {
    /// 打开设置页时重新读取统计、最近的来源应用、列表与登录启动状态。
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
    /// 绘制设置页：可滚动的各个分组，上面浮着玻璃标题栏。
    ///
    /// 控件改的是设置的一份副本，绘制结束后与原设置比较，有变化才保存并应用。
    pub(super) fn settings_ui(&mut self, ui: &mut Ui) {
        let palette = self.palette(ui.ctx());
        let escape = self.settings_view.recording_hotkey.is_none()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape));
        // 标题栏浮在内容上面（与面板的顶部栏一样是玻璃，见 `glass`）：内容从窗口顶部开始，
        // 上面留出标题栏的高度（上一帧量出来的），滚到标题栏下面的部分不直接画。
        let window = ui.max_rect().expand(f32::from(style::PAD));
        let top = self.settings_view.header_height;
        let area = ui.max_rect();
        let mut settings = self.settings.clone();
        let mut open_url = None;
        let mut source = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_rect(area.with_min_y(window.top() + top))
            .show(ui, |ui| {
                let clip = ui
                    .clip_rect()
                    .intersect(Rect::everything_below(window.top() + top));
                ui.set_clip_rect(clip);
                let layer = ui.layer_id();
                let start = glass::Source::shape_count(ui.ctx(), layer);
                ui.add_space((top - (area.top() - window.top()) - 12.0).max(0.0));
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
                section(ui, &palette, "文件搜索", |ui| {
                    self.search_section(ui, &palette, &mut settings)
                });
                let snip = section(ui, &palette, "截屏与贴图", |ui| {
                    self.snip_section(ui, &palette, &mut settings)
                });
                if std::mem::take(&mut self.settings_view.reveal_snip) {
                    let rect = snip.rect.with_min_y(snip.rect.top() - top - 8.0);
                    ui.scroll_to_rect(rect, Some(Align::Min));
                }
                section(ui, &palette, "记录", |ui| {
                    record_section(ui, &palette, &mut settings)
                });
                section(ui, &palette, "隐私与排除规则", |ui| {
                    self.rules_section(ui, &palette, &mut settings)
                });
                let tabs = section(ui, &palette, "分类标签", |ui| {
                    self.tabs_section(ui, &palette, &mut settings)
                });
                if std::mem::take(&mut self.settings_view.reveal_tabs) {
                    // 滚到标题栏下面露出来的位置，而不是被标题栏挡住的视口顶部。
                    let rect = tabs.rect.with_min_y(tabs.rect.top() - top - 8.0);
                    ui.scroll_to_rect(rect, Some(Align::Min));
                }
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
                let end = glass::Source::shape_count(ui.ctx(), layer);
                source = Some(glass::Source {
                    layer,
                    shapes: start..end,
                });
            });
        if let Some(liquid) = &self.liquid {
            liquid.capture(ui.ctx(), source.as_ref(), window, palette.background);
        }
        self.settings_header(ui, &palette, window, source.as_ref(), escape);
        if let Some(url) = open_url {
            // 面板浮在其他窗口之上，打开浏览器后收起，免得挡住网页。
            workspace::open_url(&url);
            self.hide_panel(false);
        }

        if settings != self.settings {
            let changed: Vec<HotkeyAction> = HotkeyAction::ALL
                .into_iter()
                .filter(|action| settings.hotkey(*action) != self.settings.hotkey(*action))
                .collect();
            self.settings = settings;
            for action in changed {
                self.apply_hotkey(action);
            }
            self.persist_settings();
            self.sync_middle_click();
        }
    }

    /// 浮在内容上面的标题栏：返回按钮与“设置”。
    fn settings_header(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        window: Rect,
        source: Option<&glass::Source>,
        escape: bool,
    ) {
        egui::Area::new(ui.id().with("settings-header"))
            .order(egui::Order::Middle)
            .fixed_pos(window.min)
            .constrain(false)
            .show(ui.ctx(), |ui| {
                ui.set_width(window.width());
                let bar = glass::Bar::begin(ui.painter());
                let last = Rect::from_min_size(
                    window.min,
                    Vec2::new(window.width(), self.settings_view.header_height),
                );
                drag_window(ui, last);
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: style::PAD,
                        right: style::PAD,
                        top: style::PAD,
                        bottom: 6,
                    })
                    .show(ui, |ui| {
                        centered_row(ui, 30.0, |ui| {
                            if icon_button(ui, Icon::Back, false, palette).clicked() || escape {
                                self.cancel_hotkey_recording();
                                self.set_page(Page::Panel);
                                self.panel.reset_for_show();
                            }
                            ui.label(
                                RichText::new("设置")
                                    .size(15.0)
                                    .family(egui::FontFamily::Name(fonts::SEMIBOLD.into()))
                                    .color(palette.text),
                            );
                        });
                    });
                let height = ui.min_rect().bottom() - window.top();
                let rect = Rect::from_min_size(window.min, Vec2::new(window.width(), height));
                let liquid = self.liquid.as_ref().map(|liquid| (liquid, 0));
                let tint = glass::tint(palette, liquid.is_some());
                bar.finish(ui.painter(), rect, glass::Edge::Top, source, tint, liquid);
                if (height - self.settings_view.header_height).abs() > 0.5 {
                    self.settings_view.header_height = height;
                    ui.ctx().request_discard("标题栏的高度变了");
                }
            });
    }

    /// 离开设置页时若仍在录制，恢复原快捷键的注册。
    pub(super) fn cancel_hotkey_recording(&mut self) {
        if let Some(action) = self.settings_view.recording_hotkey.take() {
            self.apply_hotkey(action);
        }
    }

    /// 按当前设置重新注册某个全局快捷键，失败时把原因显示在快捷键一行下面。
    pub(in crate::ui) fn apply_hotkey(&mut self, action: HotkeyAction) {
        // 文件搜索关闭时注销它的热键，避免关掉功能还占着组合键（§4.3）。
        let enabled = action != HotkeyAction::Search || self.settings.search_enabled;
        self.hotkey_errors[action.index()] = match &mut self.hotkeys {
            Some(service) => {
                if enabled {
                    service.apply(action, self.settings.hotkey(action)).err()
                } else {
                    service.suspend(action);
                    None
                }
            }
            None => Some("全局快捷键服务不可用".into()),
        };
    }
}
