//! Service：唯一设置所有者与命令处理（§7.10、§7.5）。
//!
//! UI 只持有表单草稿与只读视图，所有修改都经 [`Command`] 提交；
//! 校验、原子保存、派生配置与根同步都在这里串行完成。查询/扫描的异步结果带着会话与代际，
//! [`Service::poll`] 丢弃过期结果。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use devtools_search_core::scan::ExcludeSet;
use devtools_search_core::volumes::{validate_new_root, RootState};
use devtools_search_core::{RootId, RootLocation, SortColumn, VolumeId};

use super::config;
use super::protocol::{Command, Event, RootStatus};
use super::runtime::{QuerySpec, RootSlot, Runtime};
use crate::settings::{
    SearchSettings, SearchSettingsStore, SearchWindowMode, SettingsWriter, SETTINGS_VERSION,
};
use crate::{FolderPurpose, HostServices};

/// 索引变化之后等这么久再重跑查询。
///
/// 扫描和文件监听都是一连串的变化（revision 一直在涨），每变一次就重跑等于"边扫边查"：
/// 大索引一次查询要一秒，界面会被拖成幻灯片——框选、滚动时尤其明显。
const RERUN_SETTLE: Duration = Duration::from_millis(400);
/// 两次重跑之间的最小间隔（再稳一稳，别让查询把 CPU 吃满）。
const RERUN_MIN_GAP: Duration = Duration::from_millis(600);

/// 搜索模块的生命周期（§7.5）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    /// 首次使用的权限/同意引导（未完成）。
    AwaitingConsent,
    /// 已就绪但未开始建立索引（lazy_start）。
    Dormant,
    /// 正在/已经建立索引。
    Indexing,
}

/// 窗口行为策略（§5.2：Floating 失焦可隐藏；Pinned 失焦保留）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowPolicy {
    pub mode: SearchWindowMode,
    pub hide_on_blur: bool,
}

/// 一次查询的参数（用于索引变化后自动重跑）。
#[derive(Clone, Debug)]
struct LastQuery {
    text: String,
    literal: bool,
    case_sensitive: bool,
    scope: Option<PathBuf>,
}

/// `poll` 的返回。
pub struct ServicePoll {
    pub events: Vec<Event>,
    /// 窗口策略变化（UI 需要重新应用最小尺寸/失焦策略）。
    pub policy_changed: bool,
    pub repaint_after: Option<Duration>,
}

/// 搜索服务。
pub struct Service {
    host: Arc<dyn HostServices>,
    writer: SettingsWriter,
    settings: SearchSettings,
    read_only: bool,
    runtime: Runtime,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    session: u64,
    generation: u64,
    cancel: Arc<AtomicBool>,
    results: Arc<Vec<devtools_search_core::ResultId>>,
    last_query: Option<LastQuery>,
    query_in_flight: bool,
    last_total: usize,
    last_took_ms: u64,
    last_error: Option<String>,
    last_notice: Option<String>,
    lifecycle: Lifecycle,
    indexed_fingerprints: BTreeMap<RootId, (u32, u64, u32)>,
    observed_fingerprints: BTreeMap<RootId, (u32, u64, u32)>,
    /// 第一次发现索引变化的时间（用来等索引稳定下来再重查）。
    stale_since: Option<Instant>,
    /// 上一次因为索引变化重跑查询的时间（限流用）。
    last_rerun: Option<Instant>,
    /// 上次同步监听时的（排除规则, 各根状态）。没变就不要再编译一遍 glob、也不要重复挂监听。
    watcher_sync: Option<(Vec<String>, bool, Vec<(RootId, RootState, u32)>)>,
    visible: Option<bool>,
    policy_dirty: bool,
    stopped: bool,
}

impl Service {
    /// 创建服务：加载设置、建立运行时，并发出 `SettingsLoaded`。
    pub fn new(data_dir: &Path, host: Arc<dyn HostServices>) -> Self {
        let search_dir = data_dir.join("search");
        let store = SearchSettingsStore::new(&search_dir);
        let loaded = store.load();
        let mut settings = loaded.settings;
        let read_only = loaded.read_only;
        // 旧版本在 Windows 上把 canonicalize 的 `\\?\F:\…` 存进了根路径与范围，
        // 加载后统一简化并回写，界面不再出现逐字节前缀。
        if !read_only && simplify_saved_paths(&mut settings) {
            if let Err(problems) = store.save(&settings) {
                tracing::warn!("简化旧路径后保存设置失败：{problems:?}");
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint_host = host.clone();
        let runtime = Runtime::new(
            search_dir.join("snapshots"),
            tx.clone(),
            Arc::new(move || repaint_host.request_repaint()),
        );
        let lifecycle = if settings.permission_onboarding_completed {
            Lifecycle::Dormant
        } else {
            Lifecycle::AwaitingConsent
        };
        // 先装好根槽位（不扫描），让空态能显示根的形态。
        let mut service = Self {
            host,
            writer: SettingsWriter::new(store, settings.clone()),
            settings,
            read_only,
            runtime,
            tx: tx.clone(),
            rx,
            session: 0,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            results: Arc::default(),
            last_query: None,
            query_in_flight: false,
            last_total: 0,
            last_took_ms: 0,
            last_error: None,
            last_notice: None,
            lifecycle,
            indexed_fingerprints: BTreeMap::new(),
            observed_fingerprints: BTreeMap::new(),
            stale_since: None,
            last_rerun: None,
            watcher_sync: None,
            visible: None,
            policy_dirty: false,
            stopped: false,
        };
        service.sync_roots();
        service.emit_roots();
        let settings = service.settings.clone();
        let _ = service.tx.send(Event::SettingsLoaded {
            settings: Box::new(settings),
            notice: loaded.notice,
            read_only: loaded.read_only,
        });
        service
    }

    pub fn settings(&self) -> &SearchSettings {
        &self.settings
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    pub(crate) fn has_query(&self) -> bool {
        self.last_query.is_some()
    }

    pub fn results_len(&self) -> usize {
        self.results.len()
    }

    pub fn total(&self) -> usize {
        self.last_total
    }

    pub fn took_ms(&self) -> u64 {
        self.last_took_ms
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn last_notice(&self) -> Option<&str> {
        self.last_notice.as_deref()
    }

    pub fn window_policy(&self) -> WindowPolicy {
        WindowPolicy {
            mode: self.settings.ui.window.mode,
            hide_on_blur: self.settings.ui.window.mode == SearchWindowMode::Floating,
        }
    }

    pub fn roots(&self) -> Vec<RootStatus> {
        self.runtime.roots_status()
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// 是否可以开始建索引（生命周期就绪且未暂停）。
    fn indexing_allowed(&self) -> bool {
        self.settings.permission_onboarding_completed && !self.settings.index_paused
    }

    fn work_allowed(&self) -> bool {
        self.indexing_allowed()
            && (self.visible != Some(false) || self.settings.continue_when_hidden)
    }

    /// 处理一条命令（串行）。
    pub fn handle(&mut self, command: Command) {
        if self.stopped {
            return;
        }
        // 命令的结果通过 `poll()` 回到界面：宿主需要再跑一帧。
        // 搜索窗口是独立的延迟视口，子窗口的重绘不会唤醒父窗口，所以每条命令都主动唤醒宿主；
        // 重复的请求会被事件循环合并。
        self.host.request_repaint();
        match command {
            Command::WarmUp => self.warm_up(),
            Command::StartIndexing => self.start_indexing(),
            Command::Search {
                text,
                literal,
                case_sensitive,
                scope,
            } => self.search(text, literal, case_sensitive, scope, true),
            Command::LoadRows { start, count } => {
                let rows = self.runtime.load_rows(&self.results, start, count);
                let _ = self.tx.send(Event::Rows { start, rows });
            }
            Command::SetSort { column, dir } => self.set_sort(column, dir),
            Command::SetView { view } => self.set_view(view),
            Command::SetKind { kind } => self.set_kind(kind),
            Command::ApplySettings { draft } => self.apply_settings(*draft),
            Command::PickFolder => self.pick_folder(),
            Command::PickScope => self.pick_scope(),
            Command::RemoveRoot { root } => self.remove_root(root),
            Command::RebuildRoot { root } => self.rebuild_root(Some(root)),
            Command::RebuildAll => self.rebuild_root(None),
            Command::Open { path } => {
                if let Err(err) = self.host.open(&path) {
                    self.notice(format!("无法打开 {}：{err}", path.display()));
                }
            }
            Command::Reveal { path } => {
                if let Err(err) = self.host.reveal(&path) {
                    self.notice(format!("无法显示 {}：{err}", path.display()));
                }
            }
            Command::PauseIndex => self.set_paused(true),
            Command::ResumeIndex => self.set_paused(false),
            Command::ClearCache => self.clear_cache(),
            Command::ClearHistory => {
                self.settings.history.clear();
                self.persist();
                self.notice("已清空搜索历史");
            }
            Command::RememberSize { size } => {
                if size != self.settings.ui.window.size
                    && size[0] >= 800.0
                    && size[1] >= 480.0
                    && size[0] < 20_000.0
                    && size[1] < 20_000.0
                {
                    self.settings.ui.window.size = size;
                    self.persist();
                }
            }
            Command::SetMode { mode } => {
                if self.settings.ui.window.mode != mode {
                    self.settings.ui.window.mode = mode;
                    self.persist();
                    self.policy_dirty = true;
                }
            }
            Command::VisibilityChanged { visible } => self.set_visible(visible),
            Command::Shutdown => {
                self.cancel.store(true, Ordering::Release);
                self.runtime.shutdown();
                if let Err(err) = self.writer.flush() {
                    tracing::warn!("退出时保存搜索历史失败：{err}");
                }
                self.stopped = true;
                let _ = self.tx.send(Event::Stopped);
            }
        }
    }

    /// 预热（§7.5）：未完成引导时只显示根状态；lazy_start 时等首次搜索再建索引。
    fn warm_up(&mut self) {
        if self.lifecycle == Lifecycle::AwaitingConsent {
            self.emit_roots();
            return;
        }
        if !self.settings.lazy_start {
            self.start_indexing();
        } else {
            self.emit_roots();
        }
    }

    /// 开始建立索引（生命周期 → Indexing，扫描全部 Dormant 的根）。
    fn start_indexing(&mut self) {
        if !self.work_allowed() || self.lifecycle == Lifecycle::Indexing {
            return;
        }
        self.lifecycle = Lifecycle::Indexing;
        self.rescan_all();
    }

    fn search(
        &mut self,
        text: String,
        literal: bool,
        case_sensitive: bool,
        scope: Option<PathBuf>,
        record: bool,
    ) {
        // lazy_start：首次搜索才建立索引。
        if self.lifecycle == Lifecycle::Dormant && self.indexing_allowed() {
            self.start_indexing();
        }
        // 范围路径规范化（与根一致）：macOS 的 /var、/tmp 是符号链接，
        // 不规范化会和索引根的绝对路径对不上，导致"不在索引范围内"；
        // Windows 的 canonicalize 会加 `\\?\` 前缀，展示前去掉。
        let scope = if record {
            scope.map(|path| {
                crate::platform::fs::simplify(std::fs::canonicalize(&path).unwrap_or(path))
            })
        } else {
            scope
        };
        // 取消上一次查询（§7.10：新查询使旧任务尽快退出）。
        self.cancel.store(true, Ordering::SeqCst);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.session += 1;
        self.generation = self.runtime.next_generation();
        let sort = self.settings.ui.sort;
        let spec = QuerySpec {
            text: text.clone(),
            literal,
            case_sensitive,
            scope: scope.clone(),
            kind: self.settings.ui.kind,
            max_results: self.settings.max_results,
            show_all_on_empty: self.settings.show_all_on_empty,
            sort_column: sort.column,
            sort_dir: sort.dir(),
            home: directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()),
        };
        self.last_error = None;
        self.last_notice = None;
        self.last_query = Some(LastQuery {
            text: text.clone(),
            literal,
            case_sensitive,
            scope: scope.clone(),
        });
        if record {
            self.record_history(&text, scope, case_sensitive);
        }
        let slots = self.runtime.slots();
        self.indexed_fingerprints = slots
            .iter()
            .map(|slot| (slot.id, slot.fingerprint()))
            .collect();
        self.stale_since = None;
        self.query_in_flight = true;
        self.runtime.spawn_query(
            slots,
            spec,
            self.session,
            self.generation,
            self.cancel.clone(),
        );
    }

    /// 历史记录（§5.4：去重、上限、开关）。
    fn record_history(&mut self, text: &str, scope: Option<PathBuf>, case_sensitive: bool) {
        if self.read_only
            || !self.settings.history_enabled
            || self.settings.history_limit == 0
            || text.trim().is_empty()
        {
            return;
        }
        let entry = crate::settings::HistoryEntry {
            query: text.to_string(),
            scope,
            case_sensitive,
            at: chrono::Local::now().timestamp(),
        };
        self.settings
            .history
            .retain(|h| !(h.query == entry.query && h.scope == entry.scope));
        self.settings.history.insert(0, entry);
        self.settings
            .history
            .truncate(self.settings.history_limit as usize);
        self.writer.history(&self.settings.history);
    }

    fn set_sort(&mut self, column: SortColumn, dir: devtools_search_core::SortDir) {
        let next = crate::settings::UiSort {
            column,
            descending: dir == devtools_search_core::SortDir::Desc,
        };
        if self.settings.ui.sort == next {
            return;
        }
        self.settings.ui.sort = next;
        self.persist();
        // 跨根排序需要按新的键重新求值（§7.9：排序状态统一归 Service）。
        if let Some(last) = self.last_query.clone() {
            self.search(
                last.text,
                last.literal,
                last.case_sensitive,
                last.scope,
                false,
            );
        }
    }

    /// 切换结果区视图：只影响绘制，不重跑查询（图标视图和详情共用同一份结果集）。
    fn set_view(&mut self, view: crate::settings::ResultView) {
        if self.settings.ui.view == view {
            return;
        }
        self.settings.ui.view = view;
        self.persist();
    }

    /// 切换类型筛选：筛选是查询条件的一部分，改了要重跑当前查询。
    fn set_kind(&mut self, kind: devtools_search_core::FileKind) {
        if self.settings.ui.kind == kind {
            return;
        }
        self.settings.ui.kind = kind;
        self.persist();
        match self.last_query.clone() {
            Some(last) => self.search(
                last.text,
                last.literal,
                last.case_sensitive,
                last.scope,
                false,
            ),
            // 还没搜索过（例如刚打开就选了「图片」）：按"空关键词 + 该类型"查一次，
            // 首次搜索也会顺带把索引建起来。
            None => self.search(
                String::new(),
                false,
                self.settings.default_case_sensitive,
                None,
                false,
            ),
        }
    }

    fn apply_settings(&mut self, mut draft: SearchSettings) {
        draft.version = SETTINGS_VERSION;
        if self.read_only {
            let _ = self.tx.send(Event::SettingsApplied(Err(
                "当前设置文件来自更高版本，只读模式不能修改".into(),
            )));
            return;
        }
        if let Err(problems) = self.writer.save(&draft) {
            let _ = self.tx.send(Event::SettingsApplied(Err(problems)));
            return;
        }
        let old_config = config::index_config(&self.settings);
        let old_paused = self.settings.index_paused;
        let old_mode = self.settings.ui.window.mode;
        let old_sort = self.settings.ui.sort;
        let old_kind = self.settings.ui.kind;
        let was_consent = !self.settings.permission_onboarding_completed;
        self.settings = draft;
        if was_consent && self.settings.permission_onboarding_completed {
            self.lifecycle = Lifecycle::Dormant;
        }
        let new_config = config::index_config(&self.settings);
        let rules_changed = old_config.exclude_rules != new_config.exclude_rules
            || old_config.exclude_case_sensitive != new_config.exclude_case_sensitive
            || old_config.ignore_paths != new_config.ignore_paths
            || old_config.scan_app_bundles != new_config.scan_app_bundles;
        let watch_enabled = !old_config.watch && new_config.watch;
        let roots_changed = old_config.roots != new_config.roots;
        let new_slots = self.sync_roots();
        if !self.indexing_allowed() {
            if self.lifecycle == Lifecycle::Indexing {
                self.pause_all_scans();
            }
        } else if !self.work_allowed() {
            self.suspend_hidden_roots(rules_changed || watch_enabled);
        } else if self.lifecycle == Lifecycle::Indexing {
            if rules_changed || watch_enabled || old_paused {
                self.rescan_all();
            } else {
                self.scan_slots(new_slots);
                self.rescan_dirty();
            }
        } else if !self.settings.lazy_start
            && (rules_changed || roots_changed || was_consent || old_paused)
        {
            self.start_indexing();
        }
        if old_mode != self.settings.ui.window.mode {
            self.policy_dirty = true;
        }
        if old_sort != self.settings.ui.sort || old_kind != self.settings.ui.kind {
            if let Some(last) = self.last_query.clone() {
                self.search(
                    last.text,
                    last.literal,
                    last.case_sensitive,
                    last.scope,
                    false,
                );
            }
        }
        let settings = self.settings.clone();
        let _ = self.tx.send(Event::SettingsApplied(Ok(Box::new(settings))));
        self.emit_roots();
    }

    /// 添加索引根（原生选择对话框，§5.3）。
    fn pick_folder(&mut self) {
        let guard = self.host.begin_system_modal();
        let result = self.host.pick_folder(None, FolderPurpose::IndexRoot);
        if let Some(guard) = guard {
            // 面板仍在使用：对话框关闭后不还原焦点（§0）。
            guard.end(false);
        }
        match result {
            Ok(Some(path)) => self.add_root(path),
            Ok(None) => {}
            Err(err) => self.notice(err),
        }
    }

    /// 选择"在目录中搜索"的范围（原生选择对话框，§5.3）。
    fn pick_scope(&mut self) {
        let guard = self.host.begin_system_modal();
        let result = self.host.pick_folder(None, FolderPurpose::SearchScope);
        if let Some(guard) = guard {
            // 面板仍在使用：对话框关闭后不还原焦点（§0）。
            guard.end(false);
        }
        match result {
            Ok(Some(path)) => {
                // 与 search() 的规范化一致：展示与匹配用同一个形式。
                let path =
                    crate::platform::fs::simplify(std::fs::canonicalize(&path).unwrap_or(path));
                let _ = self.tx.send(Event::ScopePicked(path));
            }
            Ok(None) => {}
            Err(err) => self.notice(err),
        }
    }

    fn add_root(&mut self, path: PathBuf) {
        // 规范化后再去掉 Windows 的 `\\?\` 前缀：卷相对路径要靠 `starts_with(挂载点)` 算，
        // 带前缀时永远匹配不上，根会退化成绝对路径身份，界面也显示成 `\\?\F:\`。
        let path = crate::platform::fs::simplify(std::fs::canonicalize(&path).unwrap_or(path));
        let location = match self.location_for(&path) {
            Some(location) => location,
            None => {
                self.notice(format!("无法识别卷身份：{}", path.display()));
                return;
            }
        };
        let existing: Vec<RootLocation> = self
            .settings
            .roots
            .iter()
            .map(|r| r.location.clone())
            .collect();
        if let Err(conflict) = validate_new_root(&location, &existing) {
            self.notice(format!("无法添加 {}：{conflict}", path.display()));
            return;
        }
        let mut draft = self.settings.clone();
        draft.roots.push(crate::settings::RootConfig {
            location: location.clone(),
            enabled: true,
        });
        if let Err(problems) = self.writer.save(&draft) {
            self.notice(format!("保存设置失败：{problems}"));
            return;
        }
        self.settings = draft;
        let new_slots = self.sync_roots();
        if self.work_allowed() {
            if self.lifecycle == Lifecycle::Dormant {
                self.start_indexing();
            } else {
                self.scan_slots(new_slots);
            }
        }
        self.emit_roots();
        let settings = self.settings.clone();
        let _ = self.tx.send(Event::SettingsApplied(Ok(Box::new(settings))));
        self.notice(format!("已添加索引根：{}", path.display()));
    }

    /// 为绝对路径构造根位置（卷身份 + 卷相对路径，§7.5）。
    fn location_for(&self, path: &Path) -> Option<RootLocation> {
        use devtools_search_core::ports::VolumeSource;
        let volumes = crate::platform::volumes::StdVolumes;
        let volume_id = volumes.volume_id_of(path)?;
        // 找挂载点：取路径最长的挂载点前缀。
        let mount = volumes
            .current_mounts()
            .into_iter()
            .filter(|m| path.starts_with(&m.mount_point))
            .max_by_key(|m| m.mount_point.as_os_str().len());
        let (relative_path, fallback) = match mount {
            Some(mount) if volume_id == mount.volume_id => {
                let rel = path
                    .strip_prefix(&mount.mount_point)
                    .unwrap_or(Path::new(""))
                    .to_string_lossy()
                    .replace('\\', "/");
                (rel, path.to_path_buf())
            }
            _ => (String::new(), path.to_path_buf()),
        };
        let volume_id = if volume_id == VolumeId::fallback_absolute(path) {
            VolumeId::fallback_absolute(path)
        } else {
            volume_id
        };
        Some(RootLocation {
            volume_id,
            relative_path,
            fallback_absolute_path: fallback,
        })
    }

    fn remove_root(&mut self, root: RootId) {
        let Some(slot) = self.runtime.slot(root) else {
            return;
        };
        let identity = slot.identity.clone();
        let mut draft = self.settings.clone();
        draft.roots.retain(|r| r.location != identity);
        if let Err(problems) = self.writer.save(&draft) {
            self.notice(format!("保存设置失败：{problems}"));
            return;
        }
        self.settings = draft;
        self.runtime.remove_slot(root);
        if let Err(err) = self.runtime.remove_snapshot(&identity) {
            self.notice(format!("清理索引缓存失败：{err}"));
        }
        self.emit_roots();
        let settings = self.settings.clone();
        let _ = self.tx.send(Event::SettingsApplied(Ok(Box::new(settings))));
    }

    fn rebuild_root(&mut self, root: Option<RootId>) {
        if self.work_allowed() {
            if self.lifecycle == Lifecycle::Dormant {
                self.lifecycle = Lifecycle::Indexing;
            }
            let slots = self
                .runtime
                .slots()
                .into_iter()
                .filter(|slot| root.is_none_or(|id| id == slot.id));
            self.scan_slots(slots);
        }
    }

    fn set_paused(&mut self, paused: bool) {
        if self.settings.index_paused == paused {
            return;
        }
        self.settings.index_paused = paused;
        self.persist();
        if paused {
            self.pause_all_scans();
        } else if self.work_allowed() {
            if self.lifecycle == Lifecycle::Indexing {
                self.rescan_all();
            } else {
                self.start_indexing();
            }
        }
        self.emit_roots();
    }

    fn pause_all_scans(&self) {
        for slot in self.runtime.slots() {
            if slot.state() != RootState::Offline {
                slot.cancel_work(Some(RootState::Paused));
            }
        }
        self.emit_roots();
    }

    /// 维护就绪根的监听；已隐藏且关闭后台索引时不会被 poll 再启动。
    fn sync_watchers(&mut self) {
        let wants =
            self.settings.watch && self.work_allowed() && self.lifecycle == Lifecycle::Indexing;
        if !wants {
            if self.watcher_sync.take().is_some() {
                for slot in self.runtime.slots() {
                    self.runtime.stop_watch(&slot);
                }
            }
            return;
        }
        let signature = (
            self.settings.exclude_patterns.clone(),
            self.settings.exclude_case_sensitive,
            self.runtime
                .slots()
                .iter()
                .map(|slot| (slot.id, slot.state(), slot.fingerprint().0))
                .collect::<Vec<_>>(),
        );
        if self.watcher_sync.as_ref() == Some(&signature) {
            return;
        }
        let Ok(excludes) = ExcludeSet::compile(
            &self.settings.exclude_patterns,
            self.settings.exclude_case_sensitive,
        ) else {
            return;
        };
        for slot in self.runtime.slots() {
            self.runtime
                .spawn_watch(slot, excludes.clone(), self.settings.scan_app_bundles);
        }
        self.watcher_sync = Some(signature);
    }

    fn rescan_dirty(&self) {
        if self.work_allowed() && self.lifecycle == Lifecycle::Indexing {
            let dirty = self
                .runtime
                .slots()
                .into_iter()
                .filter(|slot| slot.state() == RootState::Dirty);
            self.scan_slots(dirty);
        }
    }

    fn rescan_all(&self) {
        self.scan_slots(self.runtime.slots());
    }

    fn scan_slots(&self, slots: impl IntoIterator<Item = Arc<RootSlot>>) {
        let mut slots = slots.into_iter().peekable();
        if slots.peek().is_none() {
            return;
        }
        let config = config::index_config(&self.settings);
        for slot in slots {
            self.runtime
                .spawn_scan(slot, &config, self.settings.persist_index);
        }
    }

    fn clear_cache(&mut self) {
        match self.runtime.clear_snapshots() {
            Ok(()) => self.notice("已清理索引缓存（下次扫描重新建立）"),
            Err(err) => self.notice(format!("清理索引缓存失败：{err}")),
        }
    }

    /// 同步已配置的根，仅返回本次新增且在线的根。
    fn sync_roots(&mut self) -> Vec<Arc<RootSlot>> {
        use devtools_search_core::ports::VolumeSource;
        let mut desired: Vec<_> = self
            .settings
            .roots
            .iter()
            .filter(|root| root.enabled)
            .map(|root| root.location.clone())
            .collect();
        for slot in self.runtime.slots() {
            if let Some(position) = desired.iter().position(|root| root == &slot.identity) {
                desired.swap_remove(position);
            } else {
                self.runtime.remove_slot(slot.id);
            }
        }
        if desired.is_empty() {
            return Vec::new();
        }
        let mounts = crate::platform::volumes::StdVolumes.current_mounts();
        let mut pending = Vec::new();
        for location in desired {
            if let Some(path) = resolve_root_path(&location, &mounts) {
                // 规范化根路径，与平台的监听路径保持一致。
                let path =
                    crate::platform::fs::simplify(std::fs::canonicalize(&path).unwrap_or(path));
                pending.push(self.runtime.add_slot(location, path));
            } else {
                let path = crate::platform::fs::simplify(location.fallback_absolute_path.clone());
                let slot = self.runtime.add_slot(location, path);
                slot.cancel_work(Some(RootState::Offline));
            }
        }
        pending
    }

    fn emit_roots(&self) {
        let _ = self.tx.send(Event::Roots {
            roots: self.runtime.roots_status(),
        });
    }

    fn notice(&self, text: impl Into<String>) {
        let _ = self.tx.send(Event::Notice(text.into()));
    }

    fn persist(&self) {
        if self.read_only {
            return;
        }
        if let Err(err) = self.writer.save(&self.settings) {
            tracing::warn!("保存搜索设置失败：{err}");
        }
    }

    /// 拉取事件，合并根状态更新并按当前索引版本失效行缓存。
    pub fn poll(&mut self) -> ServicePoll {
        let mut events = Vec::new();
        let mut roots_changed = false;
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::SearchDone(result) => {
                    if result.session != self.session {
                        continue;
                    }
                    self.query_in_flight = false;
                    self.results = result.ids.clone();
                    self.last_total = result.total;
                    self.last_took_ms = result.took_ms;
                    self.last_error = result.error.clone();
                    self.last_notice = result.notice.clone();
                    events.push(Event::SearchDone(result));
                }
                Event::Roots { .. } => roots_changed = true,
                other => events.push(other),
            }
        }
        if roots_changed {
            let current: BTreeMap<_, _> = self
                .runtime
                .slots()
                .iter()
                .map(|slot| (slot.id, slot.fingerprint()))
                .collect();
            let roots: Vec<_> = self
                .observed_fingerprints
                .iter()
                .filter(|(root, previous)| current.get(root) != Some(*previous))
                .map(|(&root, _)| root)
                .collect();
            self.observed_fingerprints = current;
            if !roots.is_empty() {
                events.push(Event::RowsInvalidated { roots });
            }
            if !self.stopped {
                self.rescan_dirty();
                self.sync_watchers();
            }
            events.push(Event::Roots {
                roots: self.runtime.roots_status(),
            });
        }
        let repaint_after = if self.stopped {
            None
        } else {
            self.rerun_if_stale(&mut events)
        };
        ServicePoll {
            events,
            policy_changed: std::mem::take(&mut self.policy_dirty),
            repaint_after,
        }
    }

    /// 索引变化合并后重查。等待时告诉宿主下一次唤醒的时间。
    fn rerun_if_stale(&mut self, events: &mut Vec<Event>) -> Option<Duration> {
        if self.visible == Some(false)
            || self.observed_fingerprints == self.indexed_fingerprints
            || self.last_query.is_none()
        {
            self.stale_since = None;
            return None;
        }

        // 首次有可查索引，即使关了自动刷新，也会补全之前在空索引上提交的查询。
        let first_index = self
            .observed_fingerprints
            .values()
            .any(|&(_, _, len)| len > 0)
            && self
                .indexed_fingerprints
                .values()
                .all(|&(_, _, len)| len == 0);
        if !self.settings.auto_refresh && !first_index {
            self.stale_since = None;
            return None;
        }
        // 正在执行的查询完成后会唤醒界面，届时一并处理期间发布的索引。
        if self.query_in_flight {
            return None;
        }

        let generation_changed =
            self.observed_fingerprints
                .iter()
                .any(|(root, &(epoch, _, len))| {
                    len > 0
                        && self
                            .indexed_fingerprints
                            .get(root)
                            .is_none_or(|&(old_epoch, _, _)| epoch != old_epoch)
                });
        let now = Instant::now();
        if !first_index && !generation_changed {
            // 监听产生的同代际更新仍做防抖；快照和完整扫描发布时立即更新。
            let first_seen = *self.stale_since.get_or_insert(now);
            let mut due = first_seen + RERUN_SETTLE;
            if let Some(last) = self.last_rerun {
                let gap = RERUN_MIN_GAP.max(Duration::from_millis(
                    self.last_took_ms.saturating_mul(3) / 2,
                ));
                due = due.max(last + gap);
            }
            if due > now {
                return Some(due - now);
            }
        }
        let last = self.last_query.clone().unwrap();
        self.last_rerun = Some(now);
        self.search(
            last.text,
            last.literal,
            last.case_sensitive,
            last.scope,
            false,
        );
        events.push(Event::ResultsStale);
        None
    }

    /// 关闭期间停止监听的根需要在重新打开后补齐这段变化。
    fn set_visible(&mut self, visible: bool) {
        self.visible = Some(visible);
        if visible && self.work_allowed() {
            if self.lifecycle == Lifecycle::Indexing {
                let pending = self.runtime.slots().into_iter().filter(|slot| {
                    matches!(
                        slot.state(),
                        RootState::Dirty | RootState::Paused | RootState::Dormant
                    )
                });
                self.scan_slots(pending);
            } else if !self.settings.lazy_start || self.last_query.is_some() {
                self.start_indexing();
            }
        } else if !visible && !self.settings.continue_when_hidden {
            self.suspend_hidden_roots(false);
        }
        self.emit_roots();
    }

    fn suspend_hidden_roots(&mut self, force_rebuild: bool) {
        if self.lifecycle != Lifecycle::Indexing {
            return;
        }
        for slot in self.runtime.slots() {
            let state = slot.state();
            let dirty = match state {
                RootState::Ready => self.settings.watch || force_rebuild,
                RootState::Scanning | RootState::PendingValidation | RootState::Verifying => true,
                _ => false,
            };
            slot.cancel_work(dirty.then_some(RootState::Dirty));
        }
        self.watcher_sync = None;
    }

    /// 关闭（取消全部任务）。
    pub fn shutdown(&mut self) {
        if !self.stopped {
            self.handle(Command::Shutdown);
        }
    }

    /// 槽位（调试/测试用）。
    pub fn slot_snapshot(&self) -> Vec<Arc<RootSlot>> {
        self.runtime.slots()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// 根位置 → 绝对路径（挂载点 + 相对路径）；卷未挂载时 `None`（§7.5 Offline）。
fn resolve_root_path(
    location: &RootLocation,
    mounts: &[devtools_search_core::ports::VolumeMount],
) -> Option<PathBuf> {
    if location.volume_id.is_fallback() {
        return location
            .fallback_absolute_path
            .is_dir()
            .then(|| location.fallback_absolute_path.clone());
    }
    let mount = mounts
        .iter()
        .find(|m| m.volume_id == location.volume_id)
        .map(|m| m.mount_point.clone())
        .or_else(|| {
            // 找不到挂载表项时退回兜底路径（卷可能在挂载表之外）。
            location
                .fallback_absolute_path
                .is_dir()
                .then(|| location.fallback_absolute_path.clone())
        })?;
    let path = if location.volume_id.is_fallback() {
        mount
    } else {
        location.absolute(&mount)
    };
    path.is_dir().then_some(path)
}

/// 把设置里保存的路径统一成展示用的形式（Windows 去掉 `\\?\` 前缀）。返回是否有改动。
fn simplify_saved_paths(settings: &mut SearchSettings) -> bool {
    use crate::platform::fs::simplify;

    let mut changed = false;
    let mut fix = |path: &mut PathBuf| {
        let plain = simplify(path.clone());
        if plain != *path {
            *path = plain;
            changed = true;
        }
    };
    for root in &mut settings.roots {
        fix(&mut root.location.fallback_absolute_path);
    }
    for entry in &mut settings.history {
        if let Some(scope) = entry.scope.as_mut() {
            fix(scope);
        }
    }
    for saved in &mut settings.saved_searches {
        if let Some(scope) = saved.scope.as_mut() {
            fix(scope);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::protocol::SearchResult;
    use crate::{settings::RootConfig, SystemModalGuard};
    use devtools_search_core::snapshot::{config_fingerprint, EventCheckpoint, SnapshotStore};
    use devtools_search_core::{root_key, NodeKind, NodeMeta, RootIndex};
    use std::ffi::OsStr;
    use std::fs;

    struct TestHost;

    impl HostServices for TestHost {
        fn pick_folder(
            &self,
            _: Option<&Path>,
            _: FolderPurpose,
        ) -> Result<Option<PathBuf>, String> {
            Ok(None)
        }
        fn begin_system_modal(&self) -> Option<Box<dyn SystemModalGuard>> {
            None
        }
        fn open(&self, _: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn reveal(&self, _: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn icon(&self, _: &Path) -> Option<(u32, u32, Vec<u8>)> {
            None
        }
        fn set_search_space_behavior(&self, _: bool) {}
        fn request_repaint(&self) {}
        fn start_window_drag(&self) -> bool {
            false
        }
    }

    fn wait_for_results(
        service: &mut Service,
        mut matches: impl FnMut(&SearchResult) -> bool,
    ) -> SearchResult {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            for event in service.poll().events {
                if let Event::SearchDone(result) = event {
                    if matches(&result) {
                        return result;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("没有收到预期的启动查询结果：{:?}", service.roots());
    }

    #[test]
    fn startup_search_shows_snapshots_before_busy_scan_workers_finish() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let search_dir = data.join("search");
        let mut settings = SearchSettings {
            roots: Vec::new(),
            lazy_start: true,
            watch: false,
            permission_onboarding_completed: true,
            history_enabled: false,
            ..SearchSettings::default()
        };
        for i in 0..3 {
            let path = dir.path().join(format!("root-{i}"));
            fs::create_dir(&path).unwrap();
            fs::write(path.join("current.txt"), "live").unwrap();
            settings.roots.push(RootConfig {
                location: RootLocation {
                    volume_id: VolumeId::fallback_absolute(&path),
                    relative_path: String::new(),
                    fallback_absolute_path: path,
                },
                enabled: true,
            });
        }
        SearchSettingsStore::new(&search_dir)
            .save(&settings)
            .unwrap();
        let store = SnapshotStore::new(search_dir.join("snapshots"));
        let fingerprint = config_fingerprint(&config::index_config(&settings));
        for root in &settings.roots {
            let mut old = RootIndex::new(RootId(0));
            let dir = old.insert(None, OsStr::new("root"), NodeKind::Dir, NodeMeta::default());
            old.insert(
                Some(dir),
                OsStr::new("previous.txt"),
                NodeKind::File,
                NodeMeta::default(),
            );
            old.commit();
            store
                .save(
                    &root_key(&root.location),
                    &old,
                    &root.location,
                    fingerprint,
                    &EventCheckpoint::default(),
                )
                .unwrap();
        }
        let mut service = Service::new(&data, Arc::new(TestHost));

        // 占满扫描池，确保即使三个根都需要全量扫描，也能先读到第三个根的快照。
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (release, unblock) = std::sync::mpsc::channel::<()>();
            let (entered, ready) = std::sync::mpsc::channel();
            service.runtime.spawn_test_scan(move || {
                entered.send(()).unwrap();
                let _ = unblock.recv();
            });
            releases.push(release);
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let manifest = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.dir().join("manifest.lock"))
            .unwrap();
        manifest.lock().unwrap();
        service.handle(Command::Search {
            text: ".txt".into(),
            scope: None,
            literal: true,
            case_sensitive: false,
        });
        wait_for_results(&mut service, |result| result.total == 0);
        drop(manifest);

        let cached = wait_for_results(&mut service, |result| result.ids.len() == 3);
        assert!(service
            .roots()
            .iter()
            .all(|root| root.from_snapshot && root.state == RootState::Verifying));
        let rows = service.runtime.load_rows(&cached.ids, 0, 3);
        assert!(rows.iter().all(|row| row.name == "previous.txt"));

        drop(releases);
        let fresh = wait_for_results(&mut service, |result| {
            result.ids.len() == 3 && result.ids.iter().all(|id| id.epoch() == 2)
        });
        let rows = service.runtime.load_rows(&fresh.ids, 0, 3);
        assert!(rows.iter().all(|row| row.name == "current.txt"));
        assert!(service
            .roots()
            .iter()
            .all(|root| !root.from_snapshot && root.state == RootState::Ready));
    }
}
