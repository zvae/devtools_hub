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

use devtools_search_core::scan::ExcludeSet;
use devtools_search_core::volumes::{validate_new_root, RootState};
use devtools_search_core::{root_key, RootId, RootLocation, SortColumn, VolumeId};

use super::config;
use super::protocol::{Command, Event, RootStatus};
use super::runtime::{QuerySpec, RootSlot, Runtime};
use crate::settings::{SearchSettings, SearchSettingsStore, SearchWindowMode, SETTINGS_VERSION};
use crate::HostServices;

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
}

/// 搜索服务。
pub struct Service {
    host: Arc<dyn HostServices>,
    store: SearchSettingsStore,
    settings: SearchSettings,
    read_only: bool,
    runtime: Runtime,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    session: u64,
    generation: u64,
    cancel: Arc<AtomicBool>,
    results: Vec<devtools_search_core::ResultId>,
    last_query: Option<LastQuery>,
    last_total: usize,
    last_took_ms: u64,
    last_error: Option<String>,
    last_notice: Option<String>,
    lifecycle: Lifecycle,
    indexed_fingerprints: BTreeMap<RootId, (u32, u64, u32)>,
    visible: bool,
    policy_dirty: bool,
    stopped: bool,
    started: bool,
}

impl Service {
    /// 创建服务：加载设置、建立运行时，并发出 `SettingsLoaded`。
    pub fn new(data_dir: &Path, host: Arc<dyn HostServices>) -> Self {
        let search_dir = data_dir.join("search");
        let store = SearchSettingsStore::new(&search_dir);
        let loaded = store.load();
        let settings = loaded.settings;
        let read_only = loaded.read_only;
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
            store,
            settings,
            read_only,
            runtime,
            tx: tx.clone(),
            rx,
            session: 0,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            results: Vec::new(),
            last_query: None,
            last_total: 0,
            last_took_ms: 0,
            last_error: None,
            last_notice: None,
            lifecycle,
            indexed_fingerprints: BTreeMap::new(),
            visible: false,
            policy_dirty: false,
            stopped: false,
            started: false,
        };
        service.sync_roots(false);
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

    /// 处理一条命令（串行）。
    pub fn handle(&mut self, command: Command) {
        if self.stopped {
            return;
        }
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
            Command::ApplySettings { draft } => self.apply_settings(*draft),
            Command::PickFolder => self.pick_folder(),
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
                if size[0] >= 800.0 && size[1] >= 480.0 && size[0] < 20_000.0 && size[1] < 20_000.0 {
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
                self.runtime.shutdown();
                self.stopped = true;
                let _ = self.tx.send(Event::Stopped);
            }
        }
    }

    /// 预热（§7.5）：未完成引导时只显示根状态；lazy_start 时等首次搜索再建索引。
    fn warm_up(&mut self) {
        self.started = true;
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
        if !self.indexing_allowed() || self.lifecycle == Lifecycle::Indexing {
            return;
        }
        self.lifecycle = Lifecycle::Indexing;
        self.sync_roots(true);
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
        // 不规范化会和索引根的绝对路径对不上，导致"不在索引范围内"。
        let scope = scope.map(|path| std::fs::canonicalize(&path).unwrap_or(path));
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
        self.runtime.spawn_query(
            self.runtime.slots(),
            spec,
            self.session,
            self.generation,
            self.cancel.clone(),
        );
    }

    /// 历史记录（§5.4：去重、上限、开关）。
    fn record_history(&mut self, text: &str, scope: Option<PathBuf>, case_sensitive: bool) {
        if !self.settings.history_enabled || text.trim().is_empty() {
            return;
        }
        let entry = crate::settings::HistoryEntry {
            query: text.to_string(),
            scope,
            case_sensitive,
            at: chrono::Local::now().timestamp(),
        };
        self.settings.history.retain(|h| !(h.query == entry.query && h.scope == entry.scope));
        self.settings.history.insert(0, entry);
        self.settings.history.truncate(self.settings.history_limit as usize);
        self.persist();
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
            self.search(last.text, last.literal, last.case_sensitive, last.scope, false);
        }
    }

    fn apply_settings(&mut self, mut draft: SearchSettings) {
        draft.version = SETTINGS_VERSION;
        if self.read_only {
            let _ = self.tx.send(Event::SettingsApplied(Err(
                "当前设置文件来自更高版本，只读模式不能修改".into()
            )));
            return;
        }
        if let Err(problems) = self.store.save(&draft) {
            let _ = self.tx.send(Event::SettingsApplied(Err(problems)));
            return;
        }
        let old_config = config::index_config(&self.settings);
        let old_mode = self.settings.ui.window.mode;
        let old_sort = self.settings.ui.sort;
        let was_consent = !self.settings.permission_onboarding_completed;
        self.settings = draft;
        if was_consent && self.settings.permission_onboarding_completed {
            self.lifecycle = Lifecycle::Dormant;
        }
        let new_config = config::index_config(&self.settings);
        let index_changed = old_config != new_config;
        self.sync_roots(false);
        if index_changed && self.indexing_allowed() {
            self.start_indexing();
        } else if self.lifecycle == Lifecycle::Indexing && self.settings.index_paused {
            self.pause_all_scans();
        }
        if old_mode != self.settings.ui.window.mode {
            self.policy_dirty = true;
        }
        if old_sort != self.settings.ui.sort {
            if let Some(last) = self.last_query.clone() {
                self.search(last.text, last.literal, last.case_sensitive, last.scope, false);
            }
        }
        let settings = self.settings.clone();
        let _ = self.tx.send(Event::SettingsApplied(Ok(Box::new(settings))));
        self.emit_roots();
    }

    /// 添加索引根（原生选择对话框，§5.3）。
    fn pick_folder(&mut self) {
        let guard = self.host.begin_system_modal();
        let result = self.host.pick_folder(None);
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

    fn add_root(&mut self, path: PathBuf) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
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
        if let Err(problems) = self.store.save(&draft) {
            self.notice(format!("保存设置失败：{problems}"));
            return;
        }
        self.settings = draft;
        self.sync_roots(false);
        if self.indexing_allowed() {
            if self.lifecycle == Lifecycle::Dormant {
                self.start_indexing();
            } else {
                self.rescan_all();
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
        if let Err(problems) = self.store.save(&draft) {
            self.notice(format!("保存设置失败：{problems}"));
            return;
        }
        self.settings = draft;
        self.runtime.cancel_scan(&slot);
        self.runtime.remove_slot(root);
        let store = devtools_search_core::snapshot::SnapshotStore::new(
            self.store.path().parent().unwrap_or(Path::new(".")).join("snapshots"),
        );
        let _ = store.remove(&root_key(&identity));
        self.emit_roots();
        let settings = self.settings.clone();
        let _ = self.tx.send(Event::SettingsApplied(Ok(Box::new(settings))));
    }

    fn rebuild_root(&mut self, root: Option<RootId>) {
        if !self.indexing_allowed() {
            return;
        }
        let config = config::index_config(&self.settings);
        let slots = self.runtime.slots();
        for slot in slots {
            if root.is_some_and(|id| id != slot.id) {
                continue;
            }
            self.runtime
                .spawn_scan(slot, config.clone(), self.settings.persist_index);
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
        } else {
            self.start_indexing();
            self.rescan_all();
        }
        self.emit_roots();
    }

    fn pause_all_scans(&mut self) {
        for slot in self.runtime.slots() {
            self.runtime.cancel_scan(&slot);
            self.runtime.stop_watch(&slot);
            *slot.state.lock().unwrap() = RootState::Paused;
        }
        self.emit_roots();
    }

    /// 维护每个根的监听：就绪的根在允许建索引且开启监听时挂上，其它情况停掉。
    fn sync_watchers(&mut self) {
        let wants = self.settings.watch
            && self.indexing_allowed()
            && self.lifecycle == Lifecycle::Indexing;
        if !wants {
            for slot in self.runtime.slots() {
                self.runtime.stop_watch(&slot);
            }
            return;
        }
        let config = config::index_config(&self.settings);
        let Ok(excludes) = ExcludeSet::compile(
            &config.exclude_rules,
            config.exclude_case_sensitive,
        ) else {
            return;
        };
        for slot in self.runtime.slots() {
            if matches!(slot.state(), RootState::Ready | RootState::Dirty) {
                self.runtime.spawn_watch(slot, excludes.clone());
            }
        }
    }

    /// 监听溢出的根转为全量重扫（`Dirty`）。
    fn rescan_dirty(&mut self) {
        if !self.indexing_allowed() || self.lifecycle != Lifecycle::Indexing {
            return;
        }
        let dirty: Vec<_> = self
            .runtime
            .slots()
            .into_iter()
            .filter(|slot| slot.state() == RootState::Dirty)
            .collect();
        if dirty.is_empty() {
            return;
        }
        let config = config::index_config(&self.settings);
        for slot in dirty {
            tracing::info!("索引需要重新校验：{}", slot.mount.display());
            self.runtime.stop_watch(&slot);
            self.runtime
                .spawn_scan(slot, config.clone(), self.settings.persist_index);
        }
    }

    fn rescan_all(&mut self) {
        let config = config::index_config(&self.settings);
        let slots = self.runtime.slots();
        for slot in slots {
            self.runtime
                .spawn_scan(slot, config.clone(), self.settings.persist_index);
        }
    }

    fn clear_cache(&mut self) {
        let dir = self
            .store
            .path()
            .parent()
            .unwrap_or(Path::new("."))
            .join("snapshots");
        let store = devtools_search_core::snapshot::SnapshotStore::new(dir);
        let keys: Vec<String> = self
            .runtime
            .slots()
            .iter()
            .map(|s| root_key(&s.identity))
            .collect();
        for key in keys {
            let _ = store.remove(&key);
        }
        self.notice("已清理索引缓存（下次扫描重新建立）");
    }

    /// 按当前设置同步根槽位：新增的根建槽位；删除的根取消并移除。
    fn sync_roots(&mut self, start_indexing: bool) {
        use devtools_search_core::ports::VolumeSource;
        let config = config::index_config(&self.settings);
        let desired: Vec<RootLocation> = config.roots.iter().map(|r| r.location.clone()).collect();
        // 移除不再需要的根。
        for slot in self.runtime.slots() {
            if !desired.contains(&slot.identity) {
                self.runtime.cancel_scan(&slot);
                self.runtime.remove_slot(slot.id);
            }
        }
        let volumes = crate::platform::volumes::StdVolumes;
        let mounts = volumes.current_mounts();
        let existing: Vec<RootId> = self.runtime.slots().iter().map(|s| s.id).collect();
        let mut pending = Vec::new();
        for location in desired {
            if self
                .runtime
                .slots()
                .iter()
                .any(|s| s.identity == location)
            {
                continue;
            }
            let resolved = resolve_root_path(&location, &mounts)
                // 规范化：macOS 的 /var、/tmp 是指向 /private/... 的符号链接，
                // 监听事件给出的是真实路径，不规范化会和 strip_prefix 对不上。
                .map(|path| std::fs::canonicalize(&path).unwrap_or(path));
            match resolved {
                Some(path) => {
                    let slot = self.runtime.add_slot(location, path);
                    pending.push(slot);
                }
                None => {
                    // 卷未挂载：保留槽位并标记 Offline，UI 显示"卷未挂载"。
                    let slot = self
                        .runtime
                        .add_slot(location.clone(), location.fallback_absolute_path.clone());
                    *slot.state.lock().unwrap() = RootState::Offline;
                }
            }
        }
        if start_indexing && self.indexing_allowed() {
            for slot in pending {
                // 只扫描还没被扫描过的根。
                if slot.state() == RootState::Dormant {
                    self.runtime.spawn_scan(
                        slot,
                        config.clone(),
                        self.settings.persist_index,
                    );
                }
            }
        } else if !self.indexing_allowed() {
            let state = if self.lifecycle == Lifecycle::AwaitingConsent {
                RootState::AwaitingConsent
            } else {
                RootState::Dormant
            };
            for id in existing {
                if let Some(slot) = self.runtime.slot(id) {
                    if slot.state() == RootState::Dormant {
                        *slot.state.lock().unwrap() = state.clone();
                    }
                }
            }
        }
        self.emit_roots();
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
        if let Err(err) = self.store.save(&self.settings) {
            tracing::warn!("保存搜索设置失败：{err}");
        }
    }

    /// 拉取事件；顺带处理会话过滤与索引变化后的自动重跑（§7.7）。
    pub fn poll(&mut self) -> ServicePoll {
        let mut events = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::SearchDone(result) => {
                    if result.session != self.session {
                        // 过期查询：丢弃（§7.10）。
                        continue;
                    }
                    self.results = result.ids.clone();
                    self.last_total = result.total;
                    self.last_took_ms = result.took_ms;
                    self.last_error = result.error.clone();
                    self.last_notice = result.notice.clone();
                    events.push(Event::SearchDone(result));
                }
                other => events.push(other),
            }
        }
        // 监听溢出 / 监听不可用之后需要全量校验的根。
        self.rescan_dirty();
        // 按开关与状态维护监听（只在索引就绪、允许建索引时监听）。
        self.sync_watchers();
        // 索引变化后重跑当前查询（避免结果集与排序长期空白/过期）。
        if self.rerun_if_stale() {
            events.push(Event::ResultsStale);
        }
        ServicePoll {
            events,
            policy_changed: std::mem::take(&mut self.policy_dirty),
        }
    }

    /// 根的 (epoch, revision, 节点数) 变化时，用当前查询参数重跑一次。
    ///
    /// 扫描完成会替换整个索引并递增 epoch，只比较 revision 发现不了（新索引从 0 开始）。
    fn rerun_if_stale(&mut self) -> bool {
        let current: BTreeMap<RootId, (u32, u64, u32)> = self
            .runtime
            .slots()
            .iter()
            .map(|slot| (slot.id, slot.fingerprint()))
            .collect();
        if current == self.indexed_fingerprints {
            return false;
        }
        self.indexed_fingerprints = current;
        let Some(last) = self.last_query.clone() else {
            return false;
        };
        if !self.started {
            return false;
        }
        self.search(last.text, last.literal, last.case_sensitive, last.scope, false);
        true
    }

    /// 搜索页显示/隐藏：`continue_when_hidden` 为 false 时隐藏即暂停扫描，显示时恢复（§6.1）。
    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if visible {
            if self.lifecycle == Lifecycle::Indexing && self.indexing_allowed() {
                let config = config::index_config(&self.settings);
                let mut resumed = false;
                for slot in self.runtime.slots() {
                    if matches!(
                        slot.state(),
                        RootState::Dirty | RootState::Paused
                    ) {
                        self.runtime
                            .spawn_scan(slot, config.clone(), self.settings.persist_index);
                        resumed = true;
                    }
                }
                if resumed {
                    self.emit_roots();
                }
            }
            return;
        }
        if self.settings.continue_when_hidden || self.lifecycle != Lifecycle::Indexing {
            return;
        }
        let mut paused = false;
        for slot in self.runtime.slots() {
            // 不继续建索引时，监听也一起停（重新显示后由 sync_watchers 恢复）。
            self.runtime.stop_watch(&slot);
            if matches!(
                slot.state(),
                RootState::Scanning | RootState::PendingValidation
            ) {
                self.runtime.cancel_scan(&slot);
                *slot.state.lock().unwrap() = RootState::Dirty;
                paused = true;
            }
        }
        if paused {
            tracing::debug!("搜索页隐藏：按设置暂停索引扫描");
            self.emit_roots();
        }
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
