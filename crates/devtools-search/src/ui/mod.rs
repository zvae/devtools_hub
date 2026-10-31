//! 搜索页 UI 与模块组合根（§4.1、§5）。
//!
//! 页面与宿主共用同一个窗口（101 层级、背景材质与面板一致）：模块只负责内容，
//! 不假设宿主页面形状，不直接展示剪贴板数据。配色与控件来自共用模块 `devtools-ui`（§5.6）。

mod cache;
mod header;
mod history;
mod icon_cache;
mod icons;
mod permissions;
mod results;
mod saved;
mod settings_panel;
mod status;
mod theme;
mod thumb_cache;
mod toolbar;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devtools_search_core::SortColumn;
use egui::Ui;

use crate::application::protocol::{Command, Event, RootStatus};
use crate::application::service::{Lifecycle, Service, WindowPolicy};
use crate::SystemMenuChoice;
use crate::settings::{ResultView, SearchSettings};
use crate::HostServices;

use cache::RowCache;
use icon_cache::IconCache;
use thumb_cache::ThumbnailCache;

/// 打开搜索页时传入的入口信息（§4.5）。
#[derive(Clone, Debug, Default)]
pub struct SearchOptions {
    /// 预填的查询文本。
    pub query: String,
    /// 选中的路径（文件夹作为范围，文件取其所在目录）。
    pub selected_paths: Vec<PathBuf>,
    /// `true`：按字面短语搜索，不解析布尔/过滤器语法。
    pub query_is_literal: bool,
    pub case_sensitive: bool,
}

/// `poll()` 的结果。
pub struct SearchPoll {
    /// 窗口策略变化（切回 `Floating` 时宿主需要恢复最小尺寸与失焦策略）。
    pub policy_changed: bool,
}

/// `ui()` 的结果。
pub struct SearchUiOutput {
    /// 用户要求收起窗口（Esc）。
    pub close: bool,
}

/// 页内子页面。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Main,
    Settings,
    Permissions,
}

/// 模块的界面状态（只在 UI 线程访问）。
pub(crate) struct ViewState {
    pub query: String,
    /// 当前查询是否是入口带来的字面短语。
    pub literal: bool,
    pub case_sensitive: bool,
    pub scope: Option<PathBuf>,
    /// 正在输入法组合（期间不发查询，§5.4）。
    pub composing: bool,
    pub last_edit: Option<Instant>,
    pub submitted: String,
    pub cursor: usize,
    /// 多选的锚点（Shift 连选时固定）。
    pub anchor: usize,
    /// 选中的结果下标。用集合而不是区间：要支持 Ctrl 点选、框选这种不连续的选法。
    pub selected: std::collections::BTreeSet<usize>,
    /// 框选起点（屏幕坐标）；`None` 表示没在拖。
    pub drag_origin: Option<egui::Pos2>,
    /// 开始框选时已经选中的项（按住 Ctrl 拖动时保留，普通拖动为空）。
    pub drag_base: std::collections::BTreeSet<usize>,
    /// 上一帧画出来的项与它的矩形（框选命中测试用）。
    pub hit_cells: Vec<(usize, egui::Rect)>,
    pub cache: RowCache,
    /// 原生文件图标的纹理缓存（按扩展名）。
    pub icons: IconCache,
    /// 预览缩略图（按路径缓存，后台线程解码）。
    pub thumbs: ThumbnailCache,
    /// 列宽（从设置复制，拖动时实时更新，松手后持久化）。
    pub columns: crate::settings::ColumnWidths,
    /// 键盘移动后要滚到的偏移。
    pub pending_scroll: Option<f32>,
    pub roots: Vec<RootStatus>,
    pub total: usize,
    pub took_ms: u64,
    pub searching: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub page: Page,
    /// 设置草稿（进入设置页时从 Service 复制）。
    pub draft: Option<SearchSettings>,
    pub settings_error: Option<String>,
    pub toast: Option<(String, Instant)>,
    pub focus_pending: bool,
    pub history_open: bool,
    /// 保存的搜索下拉。
    pub saved_open: bool,
    pub new_exclude: String,
    pub read_only: bool,
    pub lifecycle: Lifecycle,
    /// 本帧可见行范围。
    pub visible: (usize, usize),
    /// 顶栏的实际高度（含范围胶囊行），历史下拉据此定位。
    pub header_height: f32,
    /// 类型筛选下拉是否展开。
    pub kind_open: bool,
    /// 视图菜单是否展开。
    pub view_open: bool,
    /// 排序下拉是否展开。
    pub sort_open: bool,
    /// 图标视图上一帧算出的列数（键盘上下移动一整行时用）。
    pub grid_cols: usize,
    /// 搜索框当前是否有焦点（决定 ⌘A / ←→ 归输入框还是归结果区）。
    pub search_focused: bool,
    /// 右键用系统菜单还是自绘菜单（Windows 用系统菜单，macOS 自绘）。
    pub system_menus: bool,
    pub results_stale: bool,
    pub stop_requested: bool,
}

#[cfg(test)]
impl ViewState {
    /// 测试用的空状态（不引入模块与宿主）。
    fn default_for_test() -> Self {
        Self {
            query: String::new(),
            literal: false,
            case_sensitive: false,
            scope: None,
            composing: false,
            last_edit: None,
            submitted: String::new(),
            cursor: 0,
            anchor: 0,
            selected: std::collections::BTreeSet::new(),
            drag_origin: None,
            drag_base: std::collections::BTreeSet::new(),
            hit_cells: Vec::new(),
            cache: RowCache::default(),
            icons: IconCache::default(),
            thumbs: ThumbnailCache::default(),
            columns: Default::default(),
            pending_scroll: None,
            roots: Vec::new(),
            total: 0,
            took_ms: 0,
            searching: false,
            error: None,
            notice: None,
            page: Page::Main,
            draft: None,
            settings_error: None,
            toast: None,
            focus_pending: false,
            history_open: false,
            saved_open: false,
            new_exclude: String::new(),
            read_only: false,
            lifecycle: Lifecycle::Dormant,
            visible: (0, 0),
            header_height: theme::HEADER_HEIGHT,
            kind_open: false,
            view_open: false,
            sort_open: false,
            grid_cols: 1,
            search_focused: true,
            system_menus: false,
            results_stale: false,
            stop_requested: false,
        }
    }
}

impl ViewState {
    /// 某一项是否被选中。
    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.contains(&index)
    }

    /// 选中的项数。
    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    /// 只选中一项（普通点击、键盘移动）。
    pub fn select_only(&mut self, index: usize) {
        self.selected.clear();
        self.selected.insert(index);
        self.cursor = index;
        self.anchor = index;
    }

    /// Ctrl 点击：把一项加入 / 移出选择。
    pub fn toggle_selection(&mut self, index: usize) {
        if !self.selected.remove(&index) {
            self.selected.insert(index);
        }
        self.cursor = index;
        self.anchor = index;
    }

    /// Shift 点击 / Shift 方向键：把锚点到 `index` 这一段并入选择。
    pub fn extend_selection_to(&mut self, index: usize) {
        let (first, last) = (self.anchor.min(index), self.anchor.max(index));
        self.selected.extend(first..=last);
        self.cursor = index;
    }

    /// 全选。
    pub fn select_all(&mut self, rows: usize) {
        self.selected = (0..rows).collect();
        self.cursor = rows.saturating_sub(1);
        self.anchor = 0;
    }

    /// 清空选择（点空白处）。
    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.drag_origin = None;
    }

    /// 结果集变化后，把越界的下标清掉。
    pub fn clamp_selection(&mut self, rows: usize) {
        if rows == 0 {
            self.selected.clear();
            self.cursor = 0;
            self.anchor = 0;
            return;
        }
        self.selected.retain(|&index| index < rows);
        self.cursor = self.cursor.min(rows - 1);
        self.anchor = self.anchor.min(rows - 1);
    }
}

/// 搜索模块（宿主持有 `Option<SearchModule>`，见 §4.4）。
pub struct SearchModule {
    service: Service,
    pub(crate) view: ViewState,
    host: Arc<dyn HostServices>,
    last_size: [f32; 2],
    /// 第一次 `warm_up` 之后不再重复。
    warmed: bool,
}

impl SearchModule {
    /// 创建模块：打开设置、建立运行时（还不会扫描，等 `warm_up`/首次搜索）。
    pub fn new(data_dir: &std::path::Path, host: Arc<dyn HostServices>) -> Self {
        let service = Service::new(data_dir, host.clone());
        let settings = service.settings().clone();
        let lifecycle = service.lifecycle();
        let view = ViewState {
            query: String::new(),
            literal: false,
            case_sensitive: settings.default_case_sensitive,
            scope: None,
            composing: false,
            last_edit: None,
            submitted: String::new(),
            cursor: 0,
            anchor: 0,
            selected: std::collections::BTreeSet::new(),
            drag_origin: None,
            drag_base: std::collections::BTreeSet::new(),
            hit_cells: Vec::new(),
            cache: RowCache::default(),
            icons: IconCache::default(),
            thumbs: ThumbnailCache::default(),
            columns: settings.ui.columns.clone(),
            pending_scroll: None,
            roots: Vec::new(),
            total: 0,
            took_ms: 0,
            searching: false,
            error: None,
            notice: None,
            page: if lifecycle == Lifecycle::AwaitingConsent {
                Page::Permissions
            } else {
                Page::Main
            },
            draft: None,
            settings_error: None,
            toast: None,
            focus_pending: true,
            history_open: false,
            saved_open: false,
            new_exclude: String::new(),
            read_only: service.read_only(),
            lifecycle,
            visible: (0, 0),
            header_height: theme::HEADER_HEIGHT,
            kind_open: false,
            view_open: false,
            sort_open: false,
            grid_cols: 1,
            search_focused: true,
            system_menus: host.has_system_context_menu(),
            results_stale: false,
            stop_requested: false,
        };
        let last_size = settings.ui.window.size;
        Self {
            service,
            view,
            host,
            last_size,
            warmed: false,
        }
    }

    /// 预热：按 `lazy_start` / 首次引导决定是否建索引（§7.5）。
    pub fn warm_up(&mut self) {
        if self.warmed {
            return;
        }
        self.warmed = true;
        self.service.handle(Command::WarmUp);
    }

    /// 窗口在宿主中显示时调用。
    pub fn on_shown(&mut self, options: Option<SearchOptions>) {
        self.view.composing = false;
        self.view.focus_pending = true;
        self.host.set_search_space_behavior(true);
        self.service
            .handle(Command::VisibilityChanged { visible: true });
        if let Some(options) = options {
            self.apply_entry(options);
        }
    }

    /// 窗口隐藏时调用。
    pub fn on_hidden(&mut self) {
        self.view.composing = false;
        self.view.history_open = false;
        self.host.set_search_space_behavior(false);
        self.service
            .handle(Command::RememberSize { size: self.last_size });
        self.service
            .handle(Command::VisibilityChanged { visible: false });
    }

    /// 当前窗口策略。
    pub fn window_policy(&self) -> WindowPolicy {
        self.service.window_policy()
    }

    /// 宿主记录窗口尺寸（拖动边缘缩放后）。
    pub fn remember_size(&mut self, size: [f32; 2]) {
        self.last_size = size;
    }

    /// 搜索页的窗口尺寸（逻辑像素）：宿主创建窗口时使用。
    ///
    /// 返回本次会话里最新的尺寸；宿主在打开窗口时取一次，之后缩放只更新这里的值，
    /// 避免每帧把用户正在拖动的尺寸写回窗口。
    pub fn window_size(&self) -> [f32; 2] {
        self.last_size
    }

    /// 拉取事件并刷新界面状态。
    pub fn poll(&mut self) -> SearchPoll {
        let poll = self.service.poll();
        for event in poll.events {
            match event {
                Event::SettingsLoaded {
                    settings,
                    notice,
                    read_only,
                } => {
                    self.view.read_only = read_only;
                    if self.view.draft.is_none() {
                        self.view.draft = Some(*settings);
                    }
                    if let Some(notice) = notice {
                        self.toast(notice);
                    }
                    self.view.lifecycle = self.service.lifecycle();
                }
                Event::SettingsApplied(result) => match result {
                    Ok(settings) => {
                        self.view.settings_error = None;
                        self.view.columns = settings.ui.columns.clone();
                        if self.view.page == Page::Settings {
                            self.view.draft = Some(*settings);
                        }
                        self.view.lifecycle = self.service.lifecycle();
                        // 引导在别处完成（或从只读设置恢复）时，权限页要自动退出。
                        if self.view.page == Page::Permissions
                            && self.view.lifecycle != Lifecycle::AwaitingConsent
                        {
                            self.view.page = Page::Main;
                            self.view.focus_pending = true;
                        }
                        self.toast("搜索设置已保存");
                    }
                    Err(problems) => {
                        self.view.settings_error = Some(problems);
                    }
                },
                Event::Roots { roots } => {
                    self.view.roots = roots;
                    self.view.lifecycle = self.service.lifecycle();
                }
                Event::SearchDone(result) => {
                    self.view.searching = false;
                    self.view.total = result.total;
                    self.view.took_ms = result.took_ms;
                    self.view.error = result.error;
                    self.view.notice = result.notice;
                    self.view.results_stale = false;
                    // 增量应用结果集：索引更新导致的重跑只补/删变化的那几行，
                    // 不整表清空重填（否则建立索引期间表格会一直闪）。
                    let reused = self.view.cache.apply_results(&result.ids);
                    if reused == 0 {
                        // 全新的结果集（换了查询）：选中回到第一行。
                        self.view.selected.clear();
                        self.view.cursor = 0;
                        self.view.anchor = 0;
                    } else {
                        // 增量重排：只清掉越界的下标，用户选中的项尽量留着。
                        self.view.clamp_selection(result.ids.len());
                    }
                }
                Event::Rows { rows, .. } => {
                    for row in rows {
                        self.view.cache.insert(row);
                    }
                }
                Event::ResultsStale => {
                    self.view.results_stale = true;
                }
                Event::Notice(text) => self.toast(text),
                Event::ScopePicked(path) => {
                    // 顶栏的目录按钮：选完目录立即在范围内搜索（范围胶囊可点击清除）。
                    self.view.scope = Some(path);
                    self.view.history_open = false;
                    self.view.saved_open = false;
                    self.submit();
                }
                Event::Stopped => {
                    self.view.stop_requested = true;
                }
            }
        }
        if poll.policy_changed {
            self.view.cache.clear();
        }
        SearchPoll {
            policy_changed: poll.policy_changed,
        }
    }

    /// 每帧绘制。
    pub fn ui(&mut self, ui: &mut Ui) -> SearchUiOutput {
        let mut close = false;
        self.track_ime(ui);
        match self.view.page {
            Page::Permissions => {
                if let Some(choice) = permissions::show(ui, self.view.read_only) {
                    // 完成引导：写入设置；选"现在建立索引"时立即开始扫描。
                    let mut draft = self
                        .view
                        .draft
                        .clone()
                        .unwrap_or_else(|| self.service.settings().clone());
                    draft.permission_onboarding_completed = true;
                    self.view.draft = Some(draft.clone());
                    self.service
                        .handle(Command::ApplySettings { draft: Box::new(draft) });
                    if choice == permissions::PermAction::Begin {
                        self.service.handle(Command::StartIndexing);
                    }
                    self.view.page = Page::Main;
                    self.view.focus_pending = true;
                }
            }
            Page::Settings => {
                let outcome = settings_panel::show(ui, &mut self.view, &self.service);
                if outcome.pick_folder {
                    self.service.handle(Command::PickFolder);
                }
                if outcome.rebuild_all {
                    self.service.handle(Command::RebuildAll);
                }
                if outcome.clear_cache {
                    self.service.handle(Command::ClearCache);
                }
                if outcome.toggle_pause {
                    if self.service.settings().index_paused {
                        self.service.handle(Command::ResumeIndex);
                    } else {
                        self.service.handle(Command::PauseIndex);
                    }
                }
                if outcome.clear_history {
                    self.service.handle(Command::ClearHistory);
                }
                if outcome.apply {
                    if let Some(draft) = self.view.draft.clone() {
                        self.service
                            .handle(Command::ApplySettings { draft: Box::new(draft) });
                    }
                }
                if outcome.back {
                    self.view.page = Page::Main;
                    self.view.settings_error = None;
                    self.view.focus_pending = true;
                }
            }
            Page::Main => {
                // 顶栏与结果区上下紧贴（表头的分隔线就是顶栏的底边）。
                ui.spacing_mut().item_spacing.y = 0.0;
                let header = header::show(ui, &mut self.view, &self.service);
                if header.pick_scope {
                    self.service.handle(Command::PickScope);
                }
                if header.drag && !self.host.start_window_drag() {
                    // 平台层不自己拖动时（Windows）交给 winit。
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if header.toggle_case {
                    self.view.case_sensitive = !self.view.case_sensitive;
                    self.submit();
                }
                if header.toggle_history {
                    self.view.history_open = !self.view.history_open;
                    self.view.saved_open = false;
                }
                if header.toggle_saved {
                    self.view.saved_open = !self.view.saved_open;
                    self.view.history_open = false;
                }
                if header.open_settings {
                    self.open_settings();
                }
                if header.submit {
                    self.submit();
                }
                // 下拉打开时 Esc 先关下拉，不要直接收起窗口。
                if (self.view.history_open
                    || self.view.saved_open
                    || self.view.kind_open
                    || self.view.view_open
                    || self.view.sort_open)
                    && ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                    })
                {
                    self.view.history_open = false;
                    self.view.saved_open = false;
                    self.view.kind_open = false;
                    self.view.view_open = false;
                    self.view.sort_open = false;
                }
                if self.view.saved_open {
                    let saved = saved::show(ui, &mut self.view, &self.service);
                    self.handle_saved(saved);
                }
                if self.view.history_open {
                    let history = history::show(ui, &mut self.view, &self.service);
                    if history.clear {
                        self.service.handle(Command::ClearHistory);
                        self.view.history_open = false;
                    }
                    if let Some(query) = history.pick {
                        self.view.query = query;
                        self.view.literal = false;
                        self.view.history_open = false;
                        self.submit();
                    }
                }
                self.debounce(ui);
                // 「视图」菜单的快捷键：⌘⇧1…4（Windows 为 Ctrl+Shift+1…4）。
                if let Some(view) = view_shortcut(ui, self.service.settings().ui.view) {
                    // 换了视图，上一视图按行高算出的滚动偏移就不能再用了。
                    self.view.pending_scroll = None;
                    self.service.handle(Command::SetView { view });
                }
                let action = results::show(ui, &mut self.view, &self.service, &self.host);
                // 换筛选 / 换视图 / 换排序：结果集含义变了，之前选中的下标不能再当成同一批东西。
                if let Some(kind) = action.set_kind {
                    self.view.clear_selection();
                    self.service.handle(Command::SetKind { kind });
                }
                if let Some(view) = action.set_view {
                    self.view.pending_scroll = None;
                    self.view.clear_selection();
                    self.service.handle(Command::SetView { view });
                }
                // 系统右键菜单是模态的：放在结果区完全绘制之后再弹，选完才继续这一帧。
                // 顶部三条固定项走和键盘一样的命令/按键逻辑（打开、显示、复制）。
                if let Some(path) = action.system_menu {
                    match self.host.system_context_menu(&path) {
                        SystemMenuChoice::Open(path) => {
                            self.service.handle(Command::Open { path });
                        }
                        SystemMenuChoice::Reveal(path) => {
                            self.service.handle(Command::Reveal { path });
                        }
                        SystemMenuChoice::CopyPath(text) => ui.ctx().copy_text(text),
                        SystemMenuChoice::Dismissed | SystemMenuChoice::Handled => {}
                    }
                }
                if let Some((column, dir)) = action.sort {
                    self.view.clear_selection();
                    self.service.handle(Command::SetSort { column, dir });
                }
                for (start, count) in action.requests {
                    self.service.handle(Command::LoadRows { start, count });
                }
                if let Some(path) = action.open {
                    self.service.handle(Command::Open { path });
                }
                if let Some(path) = action.reveal {
                    self.service.handle(Command::Reveal { path });
                }
                if let Some(text) = action.copy {
                    ui.ctx().copy_text(text);
                }
                if action.start_indexing {
                    self.service.handle(Command::StartIndexing);
                    self.submit();
                }
                if let Some(columns) = action.column_resize {
                    let mut draft = self.service.settings().clone();
                    draft.ui.columns = columns;
                    self.view.columns = draft.ui.columns.clone();
                    self.service
                        .handle(Command::ApplySettings { draft: Box::new(draft) });
                }
                if action.close {
                    close = true;
                }
                let status = status::show(ui, &mut self.view, &self.service);
                // 状态栏下面留一点空白，不然文字和胶囊贴着窗口底边（Windows 上还会被裁）。
                ui.add_space(theme::BOTTOM_PAD);
                if status.toggle_mode {
                    let next = self.service.window_policy().mode.next();
                    self.service.handle(Command::SetMode { mode: next });
                }
            }
        }
        self.toast_ui(ui);
        SearchUiOutput { close }
    }

    /// 首次显示 / 入口切换时应用 `SearchOptions`（§4.5）。
    fn apply_entry(&mut self, options: SearchOptions) {
        if !options.query.is_empty() {
            self.view.query = options.query;
            self.view.literal = options.query_is_literal;
        }
        self.view.case_sensitive = options.case_sensitive;
        // 选中路径：文件夹作为范围，文件取所在目录。
        let scope = options
            .selected_paths
            .iter()
            .find(|p| p.is_dir())
            .cloned()
            .or_else(|| {
                options
                    .selected_paths
                    .iter()
                    .find_map(|p| p.parent().map(|parent| parent.to_path_buf()))
            });
        if scope.is_some() {
            self.view.scope = scope;
        }
        if !self.view.query.trim().is_empty() || self.view.scope.is_some() {
            self.submit();
        }
    }

    fn submit(&mut self) {
        let text = self.view.query.trim().to_string();
        // 空查询且没有范围、也不是"列出全部"时不发命令。
        if text.is_empty() && self.view.scope.is_none() && !self.service.settings().show_all_on_empty
        {
            return;
        }
        // 新查询：旧结果集里的选中项不再有意义。
        self.view.clear_selection();
        self.view.submitted = text.clone();
        self.view.searching = true;
        self.view.error = None;
        self.view.notice = None;
        self.service.handle(Command::Search {
            text,
            literal: self.view.literal,
            case_sensitive: self.view.case_sensitive,
            scope: self.view.scope.clone(),
        });
    }

    /// 防抖：输入停止 `debounce_ms` 且没有组合输入时提交。
    fn debounce(&mut self, ui: &Ui) {
        let Some(since) = self.view.last_edit else {
            return;
        };
        let debounce = Duration::from_millis(u64::from(self.service.settings().debounce_ms));
        if since.elapsed() < debounce || self.view.composing {
            return;
        }
        self.view.last_edit = None;
        self.submit();
        let _ = ui;
    }

    /// 保存的搜索：保存当前 / 应用 / 删除，全部经设置提交。
    fn handle_saved(&mut self, action: saved::SavedAction) {
        if action.save {
            let query = self.view.query.trim().to_string();
            let scope = self.view.scope.clone();
            let name = if !query.is_empty() {
                query.clone()
            } else if let Some(scope) = &scope {
                scope
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| scope.to_string_lossy().into_owned())
            } else {
                return;
            };
            let mut settings = self.service.settings().clone();
            settings
                .saved_searches
                .retain(|entry| !(entry.query == query && entry.scope == scope));
            settings.saved_searches.insert(
                0,
                crate::settings::SavedSearch {
                    name,
                    query,
                    scope,
                    case_sensitive: self.view.case_sensitive,
                },
            );
            settings.saved_searches.truncate(20);
            self.service.handle(Command::ApplySettings {
                draft: Box::new(settings),
            });
            self.view.saved_open = false;
            self.toast("已保存这条搜索");
        }
        if let Some(index) = action.delete {
            let mut settings = self.service.settings().clone();
            if index < settings.saved_searches.len() {
                settings.saved_searches.remove(index);
                self.service.handle(Command::ApplySettings {
                    draft: Box::new(settings),
                });
            }
        }
        if let Some(index) = action.apply {
            if let Some(entry) = self
                .service
                .settings()
                .saved_searches
                .get(index)
                .cloned()
            {
                self.view.query = entry.query;
                self.view.scope = entry.scope;
                self.view.case_sensitive = entry.case_sensitive;
                self.view.literal = false;
                self.view.saved_open = false;
                self.submit();
            }
        }
    }

    fn open_settings(&mut self) {
        self.view.draft = Some(self.service.settings().clone());
        self.view.settings_error = None;
        self.view.page = Page::Settings;
    }

    /// 输入法组合状态：Preedit 非空 → 组合中；Commit/空 Preedit → 结束（§5.4）。
    fn track_ime(&mut self, ui: &Ui) {
        let events = ui.input(|i| i.events.clone());
        for event in events {
            if let egui::Event::Ime(ime) = event {
                match ime {
                    egui::ImeEvent::Preedit { text, .. } => {
                        self.view.composing = !text.is_empty();
                    }
                    egui::ImeEvent::Commit(_) => self.view.composing = false,
                    _ => {}
                }
            }
        }
    }

    pub(crate) fn toast(&mut self, text: impl Into<String>) {
        self.view.toast = Some((text.into(), Instant::now() + Duration::from_millis(2400)));
    }

    fn toast_ui(&mut self, ui: &Ui) {
        let Some((text, until)) = &self.view.toast else {
            return;
        };
        if Instant::now() >= *until {
            self.view.toast = None;
            return;
        }
        let text = text.clone();
        let until = *until;
        egui::Area::new(egui::Id::new("search-toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -44.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ui.ctx(), |ui| {
                let palette = theme::palette(ui);
                egui::Frame::new()
                    .fill(if palette.dark {
                        egui::Color32::from_rgb(52, 52, 56)
                    } else {
                        egui::Color32::from_rgb(38, 38, 42)
                    })
                    .corner_radius(f32::from(theme::CARD_RADIUS))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(24)))
                    .shadow(egui::Shadow {
                        offset: [0, 6],
                        blur: 20,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(60),
                    })
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&text)
                                .color(egui::Color32::from_rgb(0xf2, 0xf2, 0xf7))
                                .size(12.5),
                        );
                    });
            });
        ui.ctx()
            .request_repaint_after(until.saturating_duration_since(Instant::now()));
    }

    /// 关闭：取消全部任务（非阻塞语义，§7.5）。
    pub fn shutdown(&mut self) {
        self.service.shutdown();
    }

    /// 有界阻塞关闭：先取消，再等待 Service 确认停止（§7.5）。
    pub fn shutdown_blocking(&mut self, timeout: Duration) -> bool {
        self.service.shutdown();
        let deadline = Instant::now() + timeout;
        while !self.service.is_stopped() {
            let _ = self.service.poll();
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    pub fn is_stopped(&self) -> bool {
        self.service.is_stopped()
    }

    pub fn service(&self) -> &Service {
        &self.service
    }

    /// 直接打开模块内的设置页（托盘菜单、宿主设置里的"搜索设置…"用）。
    pub fn show_settings(&mut self) {
        self.open_settings();
    }

    /// 可变访问（宿主测试与调试用；正常 UI 只通过命令交互）。
    pub fn service_mut(&mut self) -> &mut Service {
        &mut self.service
    }
}

/// 「视图」菜单的快捷键：⌘⇧1 → 超大图标 … ⌘⇧4 → 详情（Windows 为 Ctrl+Shift+1…4）。
///
/// 返回要切换到的视图；按的是当前视图对应的组合键时返回 `None`（不做无谓的写入）。
fn view_shortcut(ui: &mut Ui, current: ResultView) -> Option<ResultView> {
    const SHORTCUTS: [(egui::Key, ResultView); 4] = [
        (egui::Key::Num1, ResultView::ExtraLarge),
        (egui::Key::Num2, ResultView::Large),
        (egui::Key::Num3, ResultView::Medium),
        (egui::Key::Num4, ResultView::Detail),
    ];
    let modifiers = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    for (key, view) in SHORTCUTS {
        if ui.input_mut(|input| input.consume_key(modifiers, key)) && view != current {
            return Some(view);
        }
    }
    None
}

/// 表头点击后的排序循环（§5.1）：升 → 降 → 默认。
pub(crate) fn next_sort(
    current: (SortColumn, bool),
    clicked: SortColumn,
) -> (SortColumn, Option<bool>) {
    if clicked == SortColumn::Path {
        return match current.0 {
            SortColumn::Path if !current.1 => (SortColumn::Path, Some(true)),
            _ => (SortColumn::Path, None),
        };
    }
    if current.0 != clicked {
        return (clicked, Some(false));
    }
    if !current.1 {
        (clicked, Some(true))
    } else {
        (clicked, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_supports_range_toggle_and_clear() {
        let mut view = ViewState::default_for_test();
        // 单选。
        view.select_only(3);
        assert!(view.is_selected(3));
        assert_eq!(view.selected_count(), 1);
        // Shift 连选：锚点 2 到 5。
        view.anchor = 2;
        view.extend_selection_to(5);
        assert_eq!(view.selected_count(), 4);
        for index in 2..=5 {
            assert!(view.is_selected(index), "第 {index} 项应被选中");
        }
        // Ctrl 切换：把 4 移出。
        view.toggle_selection(4);
        assert!(!view.is_selected(4));
        assert_eq!(view.selected_count(), 3);
        // 全选、清空。
        view.select_all(6);
        assert_eq!(view.selected_count(), 6);
        view.clear_selection();
        assert_eq!(view.selected_count(), 0);
        // 结果变少时只丢掉越界的下标。
        view.select_all(10);
        view.clamp_selection(4);
        assert_eq!(view.selected_count(), 4);
        assert!(!view.is_selected(4));
        // 空结果直接清空。
        view.clamp_selection(0);
        assert_eq!(view.selected_count(), 0);
    }

    #[test]
    fn sort_cycles_through_asc_desc_default() {
        // 文件名列：第一次升、再降、再回默认。
        assert_eq!(next_sort((SortColumn::Path, false), SortColumn::Name), (SortColumn::Name, Some(false)));
        assert_eq!(next_sort((SortColumn::Name, false), SortColumn::Name), (SortColumn::Name, Some(true)));
        assert_eq!(next_sort((SortColumn::Name, true), SortColumn::Name), (SortColumn::Name, None));
        // 换列从升序开始。
        assert_eq!(next_sort((SortColumn::Size, true), SortColumn::Modified), (SortColumn::Modified, Some(false)));
        // 路径列：升 → 降 → 默认。
        assert_eq!(next_sort((SortColumn::Path, false), SortColumn::Path), (SortColumn::Path, Some(true)));
        assert_eq!(next_sort((SortColumn::Path, true), SortColumn::Path), (SortColumn::Path, None));
    }
}
