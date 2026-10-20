//! 剪贴板历史面板。
//!
//! 布局参照 PasteNow：顶部搜索栏，分类标签，卡片式条目（左侧来源应用图标、
//! 中间内容预览、右上角时间、右下角 ⌘N 快捷键），底部显示条目总数。
//!
//! 代码按区域拆开：
//! - 本文件：面板状态（[`PanelView`]）、面板上的操作（[`Action`]）与执行操作；
//! - `bars`：浮在列表上下的玻璃顶部栏（搜索框、分类标签、提示横幅）与底部栏；
//! - `cards`：卡片列表与单张卡片的绘制；
//! - `preview`：空格 / ⌘Y 打开的大预览；
//! - `menus`：条目与分类标签的右键菜单；
//! - `shortcuts`：面板上的键盘快捷键。

mod bars;
mod cards;
mod menus;
mod preview;
mod shortcuts;

use devtools_clipboard::{
    ClipItem, ClipList, ContentKind, ItemQuery, KindFilter, FAVORITES_LIST_ID,
};
use egui::{Frame, Rect, Ui, Vec2};

use super::native_menu::{Entry, NativeMenu};
use super::{HubApp, PasteMode};
use crate::platform::{keyboard, workspace};
use crate::settings::{Settings, Tab};
use crate::update;

pub(super) use menus::tab_menu;
use preview::Preview;

/// 一次最多加载的条目数；底部的总数不受这个限制。
const QUERY_LIMIT: usize = 1000;

/// 当前选中的分类：“全部”、内置的类型分类，或用户列表。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    /// 纯文本与富文本。
    Textual,
    /// 某一种内容类型（链接、图片、文件）。
    Kind(ContentKind),
    /// 用户列表（含内置的“收藏”）。
    List(i64),
}

impl Filter {
    /// 分类标签对应的筛选条件。
    fn from_tab(tab: Tab) -> Self {
        match tab {
            Tab::Text => Filter::Textual,
            Tab::Link => Filter::Kind(ContentKind::Link),
            Tab::Image => Filter::Kind(ContentKind::Image),
            Tab::Files => Filter::Kind(ContentKind::Files),
            Tab::List(id) => Filter::List(id),
        }
    }

    /// 对应的分类标签；“全部”没有。
    fn tab(self) -> Option<Tab> {
        match self {
            Filter::All => None,
            Filter::List(id) => Some(Tab::List(id)),
            filter => Tab::BUILT_IN
                .into_iter()
                .find(|&tab| Filter::from_tab(tab) == filter),
        }
    }

    /// 转成数据库查询条件：（类型筛选，列表 id）。
    fn to_query(self) -> (Option<KindFilter>, Option<i64>) {
        match self {
            Filter::All => (None, None),
            Filter::Textual => (Some(KindFilter::Textual), None),
            Filter::Kind(kind) => (Some(KindFilter::Exact(kind)), None),
            Filter::List(id) => (None, Some(id)),
        }
    }
}

/// 面板的界面状态。每次唤起面板时由 [`PanelView::reset_for_show`] 重置搜索与选中项。
#[derive(Default)]
pub struct PanelView {
    /// 搜索框里的文字。
    pub query: String,
    pub filter: Filter,
    /// 当前条件下加载的条目（按最近使用时间倒序）。
    pub items: Vec<ClipItem>,
    /// 所有用户列表（分类标签、右键菜单“添加到列表”用）。
    pub lists: Vec<ClipList>,
    /// 当前条件下的条目总数（列表最多加载 `QUERY_LIMIT` 条）。
    pub total: i64,
    /// 选中条目在 `items` 中的下标。
    pub selected: usize,
    /// 下一帧需要按当前条件重新加载条目。
    pub needs_reload: bool,
    /// 上次加载时数据库的修订号（见 `Shared::revision`），后台记录了新内容时修订号会变。
    pub seen_revision: u64,
    /// 固定窗口：失去焦点时不自动隐藏。
    pub pinned: bool,
    /// 空格 / ⌘Y 打开的大预览。
    pub preview_open: bool,
    /// 下一帧把焦点放到搜索框。
    focus_search: bool,
    /// 下一帧把选中的卡片滚动到可见范围内。
    scroll_to_selected: bool,
    /// 列表上一帧的滚动位置与视口高度，用来判断选中的卡片是否可见。
    scroll_offset: f32,
    viewport_height: f32,
    /// 大预览里显示的文本（缓存，切换条目时重新读取）。
    preview: Option<Preview>,
    /// 新建列表弹窗里输入的名字。
    new_list_name: String,
    /// 最近弹出的右键菜单，选中菜单项后从这里取回对应的操作。
    context_menu: Option<NativeMenu<Action>>,
    /// 下一帧打开右上角的新建列表弹窗（右键菜单的“新建列表…”）。
    open_new_list: bool,
    /// 新建列表弹窗打开期间，新列表要加入的条目。
    new_list_item: Option<i64>,
    /// 浮在列表上面、下面的顶部栏与底部栏的高度（上一帧量出来的）。
    header_height: f32,
    footer_height: f32,
    /// 读取历史失败时显示在顶部栏里的错误。
    error: Option<String>,
}

impl PanelView {
    /// 唤起面板时回到初始状态：清空搜索、回到“全部”、选中第一条、焦点放到搜索框。
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

    /// 顶部显示的分类标签：“全部”在最前，其余按设置的顺序，跳过隐藏的。
    fn filters(&self, settings: &Settings) -> Vec<(String, Filter)> {
        let list_ids: Vec<i64> = self.lists.iter().map(|list| list.id).collect();
        let tabs = settings
            .ordered_tabs(&list_ids)
            .into_iter()
            .filter(|setting| setting.visible)
            .filter_map(|setting| {
                let label = tab_label(setting.tab, &self.lists)?;
                Some((label, Filter::from_tab(setting.tab)))
            });
        std::iter::once(("全部".to_string(), Filter::All))
            .chain(tabs)
            .collect()
    }

    /// Tab / ⇧Tab 切换到下一个 / 上一个分类（循环）。
    fn cycle_filter(&mut self, forward: bool, settings: &Settings) {
        let filters = self.filters(settings);
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

    /// 切换分类：回到第一条并重新加载。
    fn set_filter(&mut self, filter: Filter) {
        if self.filter != filter {
            self.filter = filter;
            self.selected = 0;
            self.scroll_to_selected = true;
            self.needs_reload = true;
        }
    }

    /// 选中第 `index` 条（超出范围时选最后一条），并滚动到可见范围内。
    fn select(&mut self, index: usize) {
        if self.items.is_empty() {
            return;
        }
        self.selected = index.min(self.items.len() - 1);
        self.scroll_to_selected = true;
    }
}

/// 面板上产生的操作，绘制结束后统一执行，避免绘制过程中借用冲突。
#[derive(Debug, PartialEq)]
pub(super) enum Action {
    /// 按指定方式粘贴条目。
    Paste(i64, PasteMode),
    Delete(i64),
    /// 把条目加入（`member` 为 true）或移出列表。
    SetInList {
        item: i64,
        list: i64,
        member: bool,
    },
    /// 新建列表；`add_item` 不为空时把该条目加入新列表，否则切换到新列表。
    CreateList {
        name: String,
        add_item: Option<i64>,
    },
    /// 打开 / 关闭选中条目的大预览。
    TogglePreview,
    /// 暂停 / 恢复记录剪贴板。
    TogglePause,
    /// 隐藏分类标签（可在设置里恢复）。
    HideTab(Tab),
    /// 打开设置页并滚动到“分类标签”分组。
    EditTabs,
    /// 弹出条目的右键菜单。
    ItemMenu(i64),
    /// 弹出分类标签的右键菜单（“全部”没有对应的标签）。
    TabMenu(Option<Tab>),
    /// 打开新建列表弹窗，新列表创建后加入 `add_item`。
    NewList {
        add_item: Option<i64>,
    },
    OpenSettings,
    /// 收起面板并切回之前的应用。
    Close,
    /// 用浏览器打开链接，然后收起面板。
    OpenUrl(String),
    /// 在文件管理器中显示文件，然后收起面板。
    Reveal(String),
    /// 把图片条目贴到屏幕上（贴图），然后收起面板。
    PinImage(i64),
    /// 请求辅助功能权限并打开系统设置。
    RequestAccessibility,
    /// 本次运行不再在面板上提示该版本。
    DismissUpdate(String),
    /// 应用内下载并安装新版本。
    InstallUpdate,
    /// 打开设置页底部的更新信息。
    ShowUpdate,
}

impl HubApp {
    /// 绘制面板：中间是卡片列表或大预览，上下浮着两个玻璃栏；产生的操作在最后统一执行。
    pub(super) fn panel_ui(&mut self, ui: &mut Ui) {
        let palette = self.palette(ui.ctx());
        let mut actions = Vec::new();
        // 菜单打开时按键交给菜单（esc 关闭菜单、在菜单的输入框里打字），不触发面板快捷键。
        if !egui::Popup::is_any_open(ui.ctx()) {
            self.panel_keys(ui.ctx(), &mut actions);
        }

        if self.panel.needs_reload {
            self.reload_items();
        }
        let window = ui.max_rect();
        // 顶部栏、底部栏浮在列表上面：列表占满整个窗口，上下留出两个栏的高度（上一帧量出来的）。
        let (top, bottom) = (self.panel.header_height, self.panel.footer_height);
        let mut list = None;
        egui::CentralPanel::default()
            .frame(Frame::NONE)
            .show(ui, |ui| {
                if self.panel.preview_open && !self.panel.items.is_empty() {
                    let inner = Rect::from_min_max(
                        window.min + Vec2::new(0.0, top),
                        window.max - Vec2::new(0.0, bottom),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                        self.preview_ui(ui, &palette, &mut actions)
                    });
                } else {
                    self.panel.preview_open = false;
                    list = self.card_list(ui, &palette, &mut actions, top, bottom);
                }
            });
        if let Some(liquid) = &self.liquid {
            liquid.capture(ui.ctx(), list.as_ref(), window, palette.background);
        }
        self.glass_header(ui, &palette, window, list.as_ref(), &mut actions);
        self.glass_footer(ui, &palette, window, list.as_ref(), &mut actions);
        if self.panel.needs_reload {
            // 搜索词或分类在顶部栏里变了：下一帧按新条件重新加载列表。
            ui.ctx().request_repaint();
        }

        self.apply_actions(actions);
    }

    /// 按当前的搜索词与分类重新从数据库加载条目、总数与列表。
    pub(super) fn reload_items(&mut self) {
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

    /// 弹出右键菜单。Windows 的自绘菜单关闭时就知道选中了哪一项，马上执行。
    fn popup_menu(&mut self, entries: Vec<Entry<Action>>) {
        let popup = NativeMenu::popup(entries, &self.ctx);
        let chosen = popup.as_ref().and_then(|(_, chosen)| chosen.clone());
        self.panel.context_menu = popup.map(|(menu, _)| menu);
        if let Some(id) = chosen {
            self.menu_command(&id);
        }
    }

    /// 在右键菜单里选中了一项。
    pub(super) fn menu_command(&mut self, id: &str) {
        let action = self
            .panel
            .context_menu
            .as_mut()
            .and_then(|menu| menu.take(id));
        if let Some(action) = action {
            self.apply_actions(vec![action]);
        }
    }

    /// 执行面板上产生的操作（绘制结束后统一执行，见 [`Action`]）。
    pub(super) fn apply_actions(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Paste(id, mode) => self.paste(id, mode),
                Action::Delete(id) => {
                    if let Err(err) = self.store.delete(id) {
                        self.show_toast(format!("删除失败：{err:#}"));
                    }
                    self.textures.forget(id);
                    self.code.forget(id);
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
                Action::HideTab(tab) => {
                    let list_ids: Vec<i64> = self.panel.lists.iter().map(|list| list.id).collect();
                    let mut tabs = self.settings.ordered_tabs(&list_ids);
                    for setting in tabs.iter_mut().filter(|setting| setting.tab == tab) {
                        setting.visible = false;
                    }
                    self.settings.tabs = tabs;
                    self.persist_settings();
                    if self.panel.filter.tab() == Some(tab) {
                        self.panel.set_filter(Filter::All);
                    }
                    self.show_toast("已隐藏，可在设置的“分类标签”中恢复".to_string());
                }
                Action::EditTabs => self.reveal_tabs(),
                Action::ItemMenu(id) => {
                    let item = self.panel.items.iter().find(|item| item.id == id).cloned();
                    if let Some(item) = item {
                        self.popup_menu(self.item_menu(&item));
                    }
                }
                Action::TabMenu(tab) => self.popup_menu(tab_menu(tab)),
                Action::NewList { add_item } => {
                    self.panel.new_list_item = add_item;
                    self.panel.open_new_list = true;
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
                Action::PinImage(id) => {
                    let image = self
                        .store
                        .snapshot(id)
                        .ok()
                        .and_then(|snapshot| crate::snip::image_from_snapshot(&snapshot));
                    match image {
                        Some(image) => {
                            self.hide_panel(true);
                            self.pin_at_cursor(image);
                        }
                        None => self.show_toast("无法读取这张图片"),
                    }
                }
                Action::RequestAccessibility => {
                    keyboard::accessibility_trusted(true);
                    keyboard::open_accessibility_settings();
                    self.hide_panel(false);
                }
                Action::DismissUpdate(version) => self.update_dismissed = Some(version),
                Action::InstallUpdate => {
                    update::install(&self.shared);
                    self.reveal_update();
                }
                Action::ShowUpdate => self.reveal_update(),
            }
        }
    }
}

/// 分类标签上显示的名字；列表已被删除时返回 `None`。
pub(super) fn tab_label(tab: Tab, lists: &[ClipList]) -> Option<String> {
    let label = match tab {
        Tab::Text => "文本".to_string(),
        Tab::Link => "链接".to_string(),
        Tab::Image => "图片".to_string(),
        Tab::Files => "文件".to_string(),
        Tab::List(id) => {
            let list = lists.iter().find(|list| list.id == id)?;
            if id == FAVORITES_LIST_ID {
                format!("★ {}", list.name)
            } else {
                list.name.clone()
            }
        }
    };
    Some(label)
}
