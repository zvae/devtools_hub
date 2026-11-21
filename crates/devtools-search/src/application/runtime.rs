//! 运行时：专用查询/扫描线程池、各根 live 索引、快照载入与取消（§7.3、§7.9、§7.11）。
//!
//! 线程模型：Service 在 UI 线程串行处理命令；扫描与查询跑在专用 rayon 池里，结果通过
//! 事件通道回传。live 索引用 `RwLock` 保护，扫描完成时整体替换（epoch+1），查询只在读锁下工作。

mod query_execution;
use query_execution::run_query;

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use devtools_search_core::ports::FsSource;
use devtools_search_core::scan::{self, ExcludeSet, ScanOptions, ScanStats};
use devtools_search_core::snapshot::{config_fingerprint, EventCheckpoint, SnapshotStore};
use devtools_search_core::volumes::RootState;
use devtools_search_core::{
    FileKind, IndexConfig, NodeId, NodeKind, NodeMeta, ResultId, RootId, RootIndex, RootLocation,
    SortColumn, SortDir,
};

use super::protocol::{Event, RootStatus, RowData, SearchResult};
use crate::platform::fs::{self, StdFs};
use crate::platform::watcher::{FsWatcher, WatchBatch};

/// 一个根的内存槽位。
pub struct RootSlot {
    pub id: RootId,
    pub identity: RootLocation,
    /// 根对应的绝对路径（挂载点 + 相对路径）。
    pub mount: PathBuf,
    pub index: RwLock<RootIndex>,
    // 状态与任务令牌用同一把锁，旧任务退出时不能清除后来启动的任务。
    work: Mutex<RootWork>,
}

struct RootWork {
    state: RootState,
    scan: Option<Arc<AtomicBool>>,
    watch: Option<Arc<AtomicBool>>,
    prepared_watcher: Option<FsWatcher>,
    watch_disabled: bool,
    stats: ScanStats,
    from_snapshot: bool,
}

fn cancel_task(task: &mut Option<Arc<AtomicBool>>) {
    if let Some(cancel) = task.take() {
        cancel.store(true, Ordering::Release);
    }
}

impl RootSlot {
    fn new(id: RootId, identity: RootLocation, mount: PathBuf) -> Self {
        Self {
            id,
            identity,
            mount,
            index: RwLock::new(RootIndex::new(id)),
            work: Mutex::new(RootWork {
                state: RootState::Dormant,
                scan: None,
                watch: None,
                prepared_watcher: None,
                watch_disabled: false,
                stats: ScanStats::default(),
                from_snapshot: false,
            }),
        }
    }

    pub fn state(&self) -> RootState {
        self.work.lock().unwrap().state.clone()
    }

    pub(super) fn cancel_work(&self, state: Option<RootState>) {
        let mut work = self.work.lock().unwrap();
        cancel_task(&mut work.scan);
        cancel_task(&mut work.watch);
        if let Some(state) = state {
            work.state = state;
        }
        let watcher = work.prepared_watcher.take();
        drop(work);
        drop(watcher);
    }

    pub fn revision(&self) -> u64 {
        self.index.read().unwrap().revision()
    }

    pub fn len(&self) -> u32 {
        self.index.read().unwrap().len()
    }

    /// (epoch, revision, 节点数)：任一变化都说明结果集需要重跑或重取（§7.7）。
    pub fn fingerprint(&self) -> (u32, u64, u32) {
        let index = self.index.read().unwrap();
        (index.epoch(), index.revision(), index.len())
    }
}

/// 一次查询的输入。
#[derive(Clone, Debug)]
pub struct QuerySpec {
    pub text: String,
    pub literal: bool,
    pub case_sensitive: bool,
    pub scope: Option<PathBuf>,
    /// 结果区的类型筛选（空查询时也生效）。
    pub kind: FileKind,
    pub max_results: u32,
    pub show_all_on_empty: bool,
    pub sort_column: SortColumn,
    pub sort_dir: SortDir,
    pub home: Option<PathBuf>,
}

/// 运行时。
pub struct Runtime {
    query_pool: rayon::ThreadPool,
    scan_pool: Arc<rayon::ThreadPool>,
    snapshot_pool: rayon::ThreadPool,
    snapshots: Arc<SnapshotStore>,
    cache_generation: Arc<AtomicU64>,
    slots: RwLock<BTreeMap<RootId, Arc<RootSlot>>>,
    events: Sender<Event>,
    repaint: Arc<dyn Fn() + Send + Sync>,
    next_id: AtomicU16,
    generation: AtomicU64,
}

impl Runtime {
    pub fn new(
        snapshot_dir: PathBuf,
        events: Sender<Event>,
        repaint: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        // 线程数：上限 4，至少 2（§7.11）；扫描池固定 2，避免与查询抢 CPU。
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let query_threads = cores.saturating_sub(1).clamp(2, 4);
        let query_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(query_threads)
            .thread_name(|i| format!("search-query-{i}"))
            .build()
            .expect("查询线程池创建失败");
        let scan_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(|i| format!("search-scan-{i}"))
            .build()
            .expect("扫描线程池创建失败");
        // 单独载入快照，磁盘遍历占满扫描线程时，其他根仍能先发布上次的索引。
        let snapshot_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .thread_name(|_| "search-snapshot".to_owned())
            .build()
            .expect("快照线程池创建失败");
        Self {
            query_pool,
            scan_pool: Arc::new(scan_pool),
            snapshot_pool,
            snapshots: Arc::new(SnapshotStore::new(snapshot_dir)),
            cache_generation: Arc::new(AtomicU64::new(0)),
            slots: RwLock::new(BTreeMap::new()),
            events,
            repaint,
            next_id: AtomicU16::new(0),
            generation: AtomicU64::new(0),
        }
    }

    pub fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn slots(&self) -> Vec<Arc<RootSlot>> {
        self.slots.read().unwrap().values().cloned().collect()
    }

    pub fn slot(&self, id: RootId) -> Option<Arc<RootSlot>> {
        self.slots.read().unwrap().get(&id).cloned()
    }

    /// 添加一个根槽位（不扫描）。同身份已存在时返回已有的。
    pub fn add_slot(&self, identity: RootLocation, mount: PathBuf) -> Arc<RootSlot> {
        let mut slots = self.slots.write().unwrap();
        if let Some(existing) = slots.values().find(|s| s.identity == identity) {
            return existing.clone();
        }
        let id = RootId(self.next_id.fetch_add(1, Ordering::SeqCst));
        let slot = Arc::new(RootSlot::new(id, identity, mount));
        slots.insert(id, slot.clone());
        slot
    }

    pub fn remove_slot(&self, id: RootId) -> Option<Arc<RootSlot>> {
        let removed = self.slots.write().unwrap().remove(&id);
        if let Some(slot) = &removed {
            slot.cancel_work(None);
        }
        removed
    }

    pub fn remove_snapshot(
        &self,
        identity: &RootLocation,
    ) -> Result<(), devtools_search_core::snapshot::SnapshotError> {
        self.snapshots
            .remove(&devtools_search_core::root_key(identity))
    }

    pub fn clear_snapshots(&self) -> Result<(), devtools_search_core::snapshot::SnapshotError> {
        // 清理之前已在执行的扫描不能重新填回旧缓存。
        self.cache_generation.fetch_add(1, Ordering::AcqRel);
        self.snapshots.clean_orphans(&[])
    }

    /// 全部根的状态（顺序稳定按 RootId）。
    pub fn roots_status(&self) -> Vec<RootStatus> {
        self.slots()
            .into_iter()
            .map(|slot| status_of(&slot))
            .collect()
    }

    /// 已有 live 索引时直接旁路重建，只有冷启动才读快照。
    pub fn spawn_scan(&self, slot: Arc<RootSlot>, config: &IndexConfig, persist: bool) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (load_snapshot, old_watcher) = {
            let mut work = slot.work.lock().unwrap();
            if work.state == RootState::Offline {
                return;
            }
            cancel_task(&mut work.scan);
            cancel_task(&mut work.watch);
            work.watch_disabled = false;
            work.scan = Some(cancel.clone());
            let load_snapshot = persist && slot.index.read().unwrap().is_empty();
            work.state = if load_snapshot {
                RootState::PendingValidation
            } else {
                RootState::Scanning
            };
            (load_snapshot, work.prepared_watcher.take())
        };
        drop(old_watcher);
        let fingerprint = config_fingerprint(config);
        let excludes = ExcludeSet::compile(&config.exclude_rules, config.exclude_case_sensitive)
            .unwrap_or_else(|_| ExcludeSet::compile(&[], false).expect("空规则总是可编译"));
        let tx = self.events.clone();
        let repaint = self.repaint.clone();
        let snapshots = self.snapshots.clone();
        let cache_generation = self.cache_generation.clone();
        let cache_at_start = cache_generation.load(Ordering::Acquire);
        let scan_app_bundles = config.scan_app_bundles;
        let key = devtools_search_core::root_key(&slot.identity);
        #[cfg(any(target_os = "macos", windows))]
        let watch = config.watch;
        let scan = {
            let slot = slot.clone();
            let cancel = cancel.clone();
            let snapshots = snapshots.clone();
            let key = key.clone();
            let tx = tx.clone();
            let repaint = repaint.clone();
            move |loaded: Option<(EventCheckpoint, u64)>| {
                {
                    let mut work = slot.work.lock().unwrap();
                    if cancel.load(Ordering::Acquire) {
                        return;
                    }
                    work.state = if work.from_snapshot {
                        RootState::Verifying
                    } else {
                        RootState::Scanning
                    };
                }
                emit_roots(&tx, &repaint, &slot);

                let opts = ScanOptions {
                    excludes,
                    scan_app_bundles,
                    root_device: StdFs.device_of(&slot.mount),
                };
                let save = |index: &RootIndex, checkpoint: &EventCheckpoint| {
                    if persist && cache_generation.load(Ordering::Acquire) == cache_at_start {
                        let result = snapshots.save_if(
                            &key,
                            index,
                            &slot.identity,
                            fingerprint,
                            checkpoint,
                            || {
                                !cancel.load(Ordering::Acquire)
                                    && cache_generation.load(Ordering::Acquire) == cache_at_start
                            },
                        );
                        if let Err(err) = result {
                            tracing::warn!("保存快照失败：{err}");
                        }
                    }
                };
                #[cfg(any(target_os = "macos", windows))]
                if let Some((previous, generation)) = loaded.as_ref() {
                    if previous.last_event_id > 0 {
                        match replay_index(&slot, &opts, &cancel, previous, watch) {
                            Ok((changed, checkpoint, watcher)) => {
                                if changed {
                                    save(&slot.index.read().unwrap(), &checkpoint);
                                } else if checkpoint.last_event_id > previous.last_event_id {
                                    if let Err(err) = snapshots.advance_checkpoint(
                                        &key,
                                        *generation,
                                        &checkpoint,
                                        || {
                                            !cancel.load(Ordering::Acquire)
                                                && cache_generation.load(Ordering::Acquire)
                                                    == cache_at_start
                                        },
                                    ) {
                                        tracing::warn!("保存事件检查点失败：{err}");
                                    }
                                }
                                {
                                    let mut work = slot.work.lock().unwrap();
                                    if cancel.load(Ordering::Acquire) {
                                        return;
                                    }
                                    work.prepared_watcher = watcher;
                                    work.from_snapshot = false;
                                    work.state = RootState::Ready;
                                    work.scan = None;
                                }
                                tracing::debug!("已增量校验索引：{}", slot.mount.display());
                                emit_roots(&tx, &repaint, &slot);
                                return;
                            }
                            Err(err) if !cancel.load(Ordering::Acquire) => {
                                tracing::debug!(
                                    "增量校验不可用，扫描 {}：{err}",
                                    slot.mount.display()
                                );
                            }
                            Err(_) => return,
                        }
                    }
                }
                #[cfg(not(any(target_os = "macos", windows)))]
                let _ = loaded;

                // 记录文件读取之前的检查点。扫描期间发生的变化可在下一次启动时回放。
                #[cfg(any(target_os = "macos", windows))]
                let journal = if persist {
                    crate::platform::journal::Journal::capture(&slot.mount)
                        .inspect_err(|err| {
                            tracing::debug!("无法获取 {} 的日志检查点：{err}", slot.mount.display());
                        })
                        .ok()
                } else {
                    None
                };
                let mut fresh = RootIndex::new(slot.id);
                fresh.set_identity(slot.identity.clone());
                let outcome = scan::scan_root(&StdFs, &slot.mount, &opts, &cancel, &mut fresh);
                if outcome.cancelled || cancel.load(Ordering::Acquire) {
                    return;
                }
                if outcome.stats.root_failed {
                    let mut work = slot.work.lock().unwrap();
                    if cancel.load(Ordering::Acquire) {
                        return;
                    }
                    work.scan = None;
                    work.state = RootState::Error("无法读取索引根目录".into());
                    drop(work);
                    emit_roots(&tx, &repaint, &slot);
                    return;
                }
                fresh.prepare_replacement(slot.id, slot.index.read().unwrap().epoch());
                #[cfg(any(target_os = "macos", windows))]
                let checkpoint = journal
                    .filter(|journal| journal.still_current(&slot.mount))
                    .map_or_else(EventCheckpoint::default, |journal| journal.checkpoint);
                #[cfg(not(any(target_os = "macos", windows)))]
                let checkpoint = EventCheckpoint::default();
                save(&fresh, &checkpoint);
                {
                    let mut work = slot.work.lock().unwrap();
                    if cancel.load(Ordering::Acquire) {
                        return;
                    }
                    *slot.index.write().unwrap() = fresh;
                    work.stats = outcome.stats;
                    work.from_snapshot = false;
                    work.state = RootState::Ready;
                    work.scan = None;
                }
                emit_roots(&tx, &repaint, &slot);
            }
        };
        if !load_snapshot {
            self.scan_pool.spawn_fifo(move || scan(None));
            return;
        }
        let scan_pool = self.scan_pool.clone();
        self.snapshot_pool.spawn_fifo(move || {
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let checkpoint = match snapshots.load_latest(&key) {
                Ok(Some(mut snapshot))
                    if snapshot.identity == slot.identity
                        && snapshot.config_fingerprint == fingerprint =>
                {
                    let (files, dirs) = count_visible(&snapshot.index);
                    {
                        let mut work = slot.work.lock().unwrap();
                        if cancel.load(Ordering::Acquire) {
                            return;
                        }
                        let mut index = slot.index.write().unwrap();
                        snapshot.index.prepare_replacement(slot.id, index.epoch());
                        *index = snapshot.index;
                        work.from_snapshot = true;
                        work.stats.files = files;
                        work.stats.dirs = dirs;
                        work.state = RootState::Verifying;
                    }
                    emit_roots(&tx, &repaint, &slot);
                    snapshot
                        .storage_generation
                        .map(|generation| (snapshot.checkpoint, generation))
                }
                Ok(_) => None,
                Err(err) => {
                    tracing::warn!("载入快照失败（{}）：{err}", slot.mount.display());
                    None
                }
            };
            scan_pool.spawn_fifo(move || scan(checkpoint));
        });
    }

    #[cfg(test)]
    pub(super) fn spawn_test_scan(&self, task: impl FnOnce() + Send + 'static) {
        self.scan_pool.spawn_fifo(task);
    }

    /// 在查询池里执行查询；完成（未取消）时发送 `SearchDone`。
    pub fn spawn_query(
        &self,
        slots: Vec<Arc<RootSlot>>,
        spec: QuerySpec,
        session: u64,
        generation: u64,
        cancel: Arc<AtomicBool>,
    ) {
        let tx = self.events.clone();
        let repaint = self.repaint.clone();
        self.query_pool.spawn(move || {
            let started = Instant::now();
            let outcome = run_query(&slots, &spec, &cancel);
            if outcome.cancelled || cancel.load(Ordering::SeqCst) {
                return;
            }
            let _ = tx.send(Event::SearchDone(SearchResult {
                session,
                generation,
                total: outcome.total,
                took_ms: started.elapsed().as_millis() as u64,
                error: outcome.error,
                notice: outcome.notice,
                ids: Arc::new(outcome.ids),
            }));
            repaint();
        });
    }

    /// 可见行在查询池读取：构造路径及等待索引读锁都不能阻塞滚动所在的 UI 线程。
    pub fn spawn_load_rows(&self, ids: Arc<Vec<ResultId>>, start: u32, count: u32, generation: u64) {
        let slots = self.slots();
        let tx = self.events.clone();
        let repaint = self.repaint.clone();
        self.query_pool.spawn_fifo(move || {
            let rows = load_rows(&slots, &ids, start, count);
            let _ = tx.send(Event::Rows { generation, start, rows });
            repaint();
        });
    }

    /// 测试中同步读取，生产界面必须走后台任务。
    #[cfg(test)]
    pub fn load_rows(&self, ids: &[ResultId], start: u32, count: u32) -> Vec<RowData> {
        load_rows(&self.slots(), ids, start, count)
    }

    /// 单根只允许当前监听令牌提交。同一根重建期间暂停监听。
    pub fn spawn_watch(&self, slot: Arc<RootSlot>, excludes: ExcludeSet, scan_app_bundles: bool) {
        let cancel = Arc::new(AtomicBool::new(false));
        let prepared = {
            let mut work = slot.work.lock().unwrap();
            if work.watch.is_some() || work.watch_disabled || work.state != RootState::Ready {
                return;
            }
            work.watch = Some(cancel.clone());
            work.prepared_watcher.take()
        };
        let tx = self.events.clone();
        let repaint = self.repaint.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("search-watch-{}", slot.id.0))
            .spawn({
                let slot = slot.clone();
                let cancel = cancel.clone();
                move || {
                    watch_root(
                        &slot,
                        excludes,
                        scan_app_bundles,
                        prepared,
                        &cancel,
                        &tx,
                        &repaint,
                    )
                }
            });
        if let Err(err) = spawned {
            stop_failed_watch(&slot, &cancel, &self.events, &self.repaint, &err);
        }
    }

    pub fn stop_watch(&self, slot: &RootSlot) {
        let mut work = slot.work.lock().unwrap();
        cancel_task(&mut work.watch);
        let watcher = work.prepared_watcher.take();
        drop(work);
        drop(watcher);
    }

    /// 取消全部扫描和监听，任务在下一个取消检查点释放根。
    pub fn shutdown(&self) {
        for slot in self.slots() {
            slot.cancel_work(None);
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct QueryOutcome {
    ids: Vec<ResultId>,
    total: usize,
    error: Option<String>,
    notice: Option<String>,
    cancelled: bool,
}

impl QueryOutcome {
    fn empty(error: Option<String>, notice: Option<String>) -> Self {
        Self {
            ids: Vec::new(),
            total: 0,
            error,
            notice,
            cancelled: false,
        }
    }

    fn cancelled() -> Self {
        Self {
            ids: Vec::new(),
            total: 0,
            error: None,
            notice: None,
            cancelled: true,
        }
    }
}

/// 每个连续的同根批次共用一次读锁，避免一批 128 行反复竞争锁。
fn load_rows(slots: &[Arc<RootSlot>], ids: &[ResultId], start: u32, count: u32) -> Vec<RowData> {
    let end = start.saturating_add(count).min(ids.len() as u32);
    let mut rows = Vec::with_capacity(end.saturating_sub(start) as usize);
    let mut cursor = start;
    while cursor < end {
        let root = ids[cursor as usize].root();
        let next = (cursor + 1..end)
            .find(|&index| ids[index as usize].root() != root)
            .unwrap_or(end);
        if let Some(slot) = slots.iter().find(|slot| slot.id == root) {
            let index = slot.index.read().unwrap();
            for display in cursor..next {
                rows.push(build_row(slot, &index, ids[display as usize], display));
            }
        } else {
            for display in cursor..next {
                rows.push(missing_row(ids[display as usize], display));
            }
        }
        cursor = next;
    }
    rows
}

fn missing_row(id: ResultId, index: u32) -> RowData {
    RowData {
        index,
        missing: true,
        result: id,
        name: String::new(),
        path: String::new(),
        parent: String::new(),
        size: None,
        mtime: None,
        btime: None,
        kind: NodeKind::File,
    }
}

/// 构造一行数据（路径按父链拼接；失效节点返回占位行，§7.1）。
fn build_row(slot: &RootSlot, index: &RootIndex, id: ResultId, display_index: u32) -> RowData {
    let node = id.node();
    if id.epoch() != index.epoch() || !index.node_id_valid(node) || !index.status(node).visible() {
        return missing_row(id, display_index);
    }
    let mut segments: Vec<String> = Vec::new();
    let mut cur = node;
    let mut guard = 0;
    loop {
        if guard > 512 {
            // 父链异常（损坏）：占位行而不是死循环。
            return missing_row(id, display_index);
        }
        guard += 1;
        match index.parent(cur) {
            Some(parent) => {
                segments.push(
                    index
                        .raw_name_str(cur)
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            String::from_utf8_lossy(index.raw_name(cur)).into_owned()
                        }),
                );
                cur = parent;
            }
            None => break,
        }
    }
    segments.reverse();
    let mut path = slot.mount.clone();
    for segment in &segments {
        path.push(segment);
    }
    let parent = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let meta = index.meta(node);
    RowData {
        index: display_index,
        missing: false,
        result: id,
        name: index
            .raw_name_str(node)
            .map(str::to_string)
            .unwrap_or_else(|| String::from_utf8_lossy(index.raw_name(node)).into_owned()),
        path: path.to_string_lossy().into_owned(),
        parent,
        size: meta.size,
        mtime: meta.mtime,
        btime: meta.btime,
        kind: index.kind(node),
    }
}

fn status_of(slot: &RootSlot) -> RootStatus {
    let work = slot.work.lock().unwrap();
    RootStatus {
        id: slot.id,
        path: slot.mount.clone(),
        state: work.state.clone(),
        files: work.stats.files,
        dirs: work.stats.dirs,
        from_snapshot: work.from_snapshot,
    }
}

fn emit_roots(tx: &Sender<Event>, repaint: &Arc<dyn Fn() + Send + Sync>, slot: &RootSlot) {
    let _ = tx.send(Event::Roots {
        roots: vec![status_of(slot)],
    });
    repaint();
}

fn stop_failed_watch(
    slot: &RootSlot,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
    repaint: &Arc<dyn Fn() + Send + Sync>,
    err: &dyn std::fmt::Display,
) {
    let mut work = slot.work.lock().unwrap();
    if cancel.load(Ordering::Acquire) {
        return;
    }
    work.watch_disabled = true;
    work.watch = None;
    drop(work);
    tracing::warn!("无法监听 {}：{err}", slot.mount.display());
    let _ = tx.send(Event::Notice(format!(
        "无法实时监听 {}：{err}。变更后请手动重建索引",
        slot.mount.display()
    )));
    repaint();
}

fn watch_root(
    slot: &RootSlot,
    excludes: ExcludeSet,
    scan_app_bundles: bool,
    prepared: Option<FsWatcher>,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
    repaint: &Arc<dyn Fn() + Send + Sync>,
) {
    if cancel.load(Ordering::Acquire) {
        return;
    }
    let mut watcher = match prepared {
        Some(watcher) => watcher,
        None => match FsWatcher::start(&slot.mount) {
            Ok(watcher) => watcher,
            Err(err) => return stop_failed_watch(slot, cancel, tx, repaint, &err),
        },
    };
    let opts = ScanOptions {
        excludes,
        scan_app_bundles,
        root_device: StdFs.device_of(&slot.mount),
    };
    while !cancel.load(Ordering::Acquire) {
        let changes = match watcher.next_batch(Duration::from_millis(350), cancel) {
            None => continue,
            Some(WatchBatch::Paths(paths)) => collect_watch_changes(slot, &opts, &paths, cancel),
            Some(WatchBatch::Overflow) => Err(io::Error::other("监听事件需要全量校验")),
        };
        let mut work = slot.work.lock().unwrap();
        if cancel.load(Ordering::Acquire) {
            break;
        }
        match changes {
            Ok(changes) => {
                let changed =
                    apply_watch_batch(&mut slot.index.write().unwrap(), &mut work.stats, changes);
                drop(work);
                if changed {
                    emit_roots(tx, repaint, slot);
                }
            }
            Err(err) => {
                tracing::info!("索引需要重新校验（{}）：{err}", slot.mount.display());
                work.state = RootState::Dirty;
                work.watch = None;
                drop(work);
                emit_roots(tx, repaint, slot);
                break;
            }
        }
    }
    watcher.stop(&slot.mount);
}

#[derive(Default)]
struct WatchChanges {
    entries: BTreeMap<PathBuf, Option<(NodeKind, NodeMeta)>>,
    replace_dirs: BTreeSet<PathBuf>,
}

#[cfg(any(target_os = "macos", windows))]
fn replay_index(
    slot: &RootSlot,
    opts: &ScanOptions,
    cancel: &AtomicBool,
    previous: &EventCheckpoint,
    watch: bool,
) -> io::Result<(bool, EventCheckpoint, Option<FsWatcher>)> {
    // 在采集检查点之前挂实时监听，覆盖检查点与切换就绪之间的事件。
    let watcher = if watch {
        Some(FsWatcher::start(&slot.mount).map_err(io::Error::other)?)
    } else {
        None
    };
    let journal = crate::platform::journal::Journal::capture(&slot.mount)?;
    let paths = journal.replay(&slot.mount, previous, cancel)?;
    let changes = collect_changes(slot, opts, paths.into_iter(), cancel)?;
    if !journal.still_current(&slot.mount) {
        return Err(io::Error::other("回放期间索引根已变化"));
    }
    let mut work = slot.work.lock().unwrap();
    if cancel.load(Ordering::Acquire) {
        return Err(io::ErrorKind::Interrupted.into());
    }
    let changed = apply_watch_batch(&mut slot.index.write().unwrap(), &mut work.stats, changes);
    Ok((changed, journal.checkpoint, watcher))
}

/// 在锁外查元数据，只对新出现的目录枚举其子树。
fn collect_watch_changes(
    slot: &RootSlot,
    opts: &ScanOptions,
    paths: &[PathBuf],
    cancel: &AtomicBool,
) -> io::Result<WatchChanges> {
    collect_changes(
        slot,
        opts,
        paths.iter().cloned().map(|path| (path, false)),
        cancel,
    )
}

fn collect_changes(
    slot: &RootSlot,
    opts: &ScanOptions,
    paths: impl IntoIterator<Item = (PathBuf, bool)>,
    cancel: &AtomicBool,
) -> io::Result<WatchChanges> {
    let mut relative_paths = BTreeMap::new();
    for (path, subtree) in paths {
        let relative = path
            .strip_prefix(&slot.mount)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| {
                let simple = fs::simplify(path.clone());
                if let Ok(relative) = simple.strip_prefix(&slot.mount) {
                    return Some(relative.to_path_buf());
                }
                fs::simplify(std::fs::canonicalize(simple).ok()?)
                    .strip_prefix(&slot.mount)
                    .ok()
                    .map(Path::to_path_buf)
            });
        if let Some(relative) = relative {
            *relative_paths.entry(relative).or_default() |= subtree;
        }
    }
    let mut changes = WatchChanges::default();
    let mut visited_subtrees: BTreeSet<PathBuf> = BTreeSet::new();
    for (path, recheck_subtree) in relative_paths {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        if path.as_os_str().is_empty() {
            if !slot.mount.is_dir() {
                return Err(io::Error::new(io::ErrorKind::NotFound, "索引根已移除"));
            }
            continue;
        }
        if path
            .ancestors()
            .any(|ancestor| visited_subtrees.contains(ancestor))
        {
            continue;
        }
        // 平台只报告子项时，从第一个未知祖先开始补齐，已有祖先不用重新 stat。
        let candidate = {
            let index = slot.index.read().unwrap();
            let mut candidate = Some(path.as_path());
            for parent in path.ancestors().skip(1) {
                match watch_node(&index, parent) {
                    Some(node)
                        if index.kind(node) == NodeKind::Dir && index.status(node).visible() =>
                    {
                        break
                    }
                    Some(node) if !index.status(node).visible() => {
                        candidate = None;
                        break;
                    }
                    Some(_) => {
                        // 交给已索引的祖先自身重新分类，避免经过符号链接或包目录。
                        candidate = Some(parent);
                        break;
                    }
                    None => candidate = Some(parent),
                }
            }
            candidate.map(Path::to_path_buf)
        };
        let Some(candidate) = candidate else {
            continue;
        };
        if changes.entries.contains_key(&candidate) || visited_subtrees.contains(&candidate) {
            continue;
        }
        let absolute = slot.mount.join(&candidate);
        let entry = match fs::entry_at(&absolute) {
            Ok(entry) => entry,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                changes.entries.insert(candidate, None);
                continue;
            }
            Err(err) => return Err(err),
        };
        let (kind, excluded, descend) = opts.classify(&entry, &relative_text(&candidate));
        if excluded {
            changes.entries.insert(candidate, None);
            continue;
        }
        if entry.device.is_some() && opts.root_device.is_some() && entry.device != opts.root_device
        {
            continue;
        }
        let existing_dir = descend && {
            let index = slot.index.read().unwrap();
            watch_node(&index, &candidate).is_some_and(|node| index.kind(node) == NodeKind::Dir)
        };
        let replace = existing_dir && recheck_subtree && path == candidate;
        changes
            .entries
            .insert(candidate.clone(), Some((kind, entry.meta)));
        if descend && (!existing_dir || replace) {
            if replace {
                changes.replace_dirs.insert(candidate.clone());
            }
            collect_subtree(&slot.mount, &candidate, opts, cancel, &mut changes.entries)?;
            visited_subtrees.insert(candidate);
        }
    }
    Ok(changes)
}

fn relative_text(path: &Path) -> String {
    path.iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn collect_subtree(
    root: &Path,
    start: &Path,
    opts: &ScanOptions,
    cancel: &AtomicBool,
    changes: &mut BTreeMap<PathBuf, Option<(NodeKind, NodeMeta)>>,
) -> io::Result<()> {
    let mut stack = vec![start.to_path_buf()];
    let mut entries = Vec::new();
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        match StdFs.list_dir(&root.join(&dir), &mut entries) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                changes.insert(dir, None);
                continue;
            }
            Err(err) => return Err(err),
        }
        for entry in &entries {
            let path = dir.join(&entry.name);
            let (kind, excluded, descend) = opts.classify(entry, &relative_text(&path));
            if excluded {
                continue;
            }
            changes.insert(path.clone(), Some((kind, entry.meta)));
            if descend {
                stack.push(path);
            }
        }
    }
    Ok(())
}

fn watch_node(index: &RootIndex, path: &Path) -> Option<NodeId> {
    path.iter().try_fold(index.root_node()?, |parent, name| {
        index.child_by_exact_name(parent, name)
    })
}

/// 先删除，再按父节点到子节点的顺序应用插入。批内新增目录由本地表解析。
fn apply_watch_batch(index: &mut RootIndex, stats: &mut ScanStats, changes: WatchChanges) -> bool {
    let mut structure_changed = false;
    let mut metadata_changed = false;
    for (path, entry) in &changes.entries {
        let Some(node) = watch_node(index, path) else {
            continue;
        };
        if entry.is_some_and(|(kind, _)| kind == index.kind(node))
            && !changes.replace_dirs.contains(path)
        {
            continue;
        }
        for node in index.subtree_nodes(node) {
            if index.status(node).visible() {
                match index.kind(node) {
                    NodeKind::File => stats.files -= 1,
                    NodeKind::Dir => stats.dirs -= 1,
                }
            }
            index.mark_deleted(node);
        }
        structure_changed = true;
    }
    let mut new_dirs = BTreeMap::new();
    for (path, entry) in changes.entries {
        let Some((kind, meta)) = entry else {
            continue;
        };
        let Some(parent_path) = path.parent() else {
            continue;
        };
        let Some(parent) = new_dirs
            .get(parent_path)
            .copied()
            .or_else(|| watch_node(index, parent_path))
            .filter(|&id| index.kind(id) == NodeKind::Dir && index.status(id).visible())
        else {
            continue;
        };
        if let Some(node) = index.child_by_exact_name(parent, path.file_name().unwrap()) {
            if index.meta(node) != meta {
                index.update_meta(node, meta);
                metadata_changed = true;
            }
            continue;
        }
        let node = index.insert(Some(parent), path.file_name().unwrap(), kind, meta);
        match kind {
            NodeKind::File => stats.files += 1,
            NodeKind::Dir => {
                stats.dirs += 1;
                new_dirs.insert(path, node);
            }
        }
        structure_changed = true;
    }
    if structure_changed {
        if index.stale_ratio() <= 0.3 || !index.compact() {
            index.commit();
        }
    } else if metadata_changed {
        index.commit_metadata();
    }
    structure_changed || metadata_changed
}

/// 统计可见的文件 / 目录数量（状态栏显示用）。
fn count_visible(index: &RootIndex) -> (u32, u32) {
    let mut files = 0;
    let mut dirs = 0;
    for node in 1..index.len() {
        let node = NodeId(node);
        if !index.status(node).visible() {
            continue;
        }
        match index.kind(node) {
            NodeKind::File => files += 1,
            NodeKind::Dir => dirs += 1,
        }
    }
    (files, dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devtools_search_core::VolumeId;
    use std::ffi::OsStr;

    fn root(id: u16, mount: PathBuf) -> RootSlot {
        let identity = RootLocation {
            volume_id: VolumeId::fallback_absolute(&mount),
            relative_path: String::new(),
            fallback_absolute_path: mount.clone(),
        };
        RootSlot::new(RootId(id), identity, mount)
    }

    #[test]
    fn loading_visible_rows_returns_without_waiting_for_index_writer() {
        use std::sync::mpsc::channel;
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = channel();
        let (repaint_tx, repaint_rx) = channel();
        let runtime = Runtime::new(dir.path().join("snapshots"), tx, Arc::new(move || {
            let _ = repaint_tx.send(());
        }));
        let mount = dir.path().join("files");
        let identity = RootLocation {
            volume_id: VolumeId::fallback_absolute(&mount),
            relative_path: String::new(),
            fallback_absolute_path: mount.clone(),
        };
        let slot = runtime.add_slot(identity, mount.clone());
        let id = {
            let mut index = slot.index.write().unwrap();
            let root = index.insert(None, OsStr::new("files"), NodeKind::Dir, NodeMeta::default());
            let node = index.insert(Some(root), OsStr::new("test.png"), NodeKind::File, NodeMeta::default());
            index.commit();
            ResultId::new(slot.id, node, index.epoch())
        };
        // 持有写锁模拟监听更新。发请求的一侧必须先返回，后台等锁释放后再交付行。
        let writer = slot.index.write().unwrap();
        let (done_tx, done_rx) = channel();
        let returned = std::thread::scope(|scope| {
            scope.spawn(|| {
                runtime.spawn_load_rows(Arc::new(vec![id]), 0, 128, 42);
                done_tx.send(()).unwrap();
            });
            let returned = done_rx.recv_timeout(Duration::from_secs(2));
            assert!(rx.try_recv().is_err());
            drop(writer);
            returned
        });
        assert!(returned.is_ok(), "取行请求不应等待索引写锁");
        let Event::Rows { generation, rows, .. } = rx.recv_timeout(Duration::from_secs(2)).unwrap() else {
            panic!("应返回行数据");
        };
        assert_eq!(generation, 42);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "test.png");
        assert_eq!(PathBuf::from(&rows[0].path), mount.join("test.png"));
        repaint_rx.recv_timeout(Duration::from_secs(2)).expect("数据到达后应唤醒界面");
    }

    #[test]
    fn watcher_batch_updates_siblings_and_preserves_structure_for_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("nested");
        std::fs::create_dir(&folder).unwrap();
        let slot = root(0, dir.path().to_path_buf());
        let opts = ScanOptions {
            excludes: ExcludeSet::compile(&[], false).unwrap(),
            scan_app_bundles: false,
            root_device: StdFs.device_of(dir.path()),
        };
        let cancel = AtomicBool::new(false);
        let mut stats = scan::scan_root(
            &StdFs,
            dir.path(),
            &opts,
            &cancel,
            &mut slot.index.write().unwrap(),
        )
        .stats;
        assert_eq!(stats.dirs, 1);

        let paths: Vec<_> = (0..6)
            .map(|i| folder.join(format!("file-{i}.txt")))
            .collect();
        for path in &paths {
            std::fs::write(path, "x").unwrap();
        }
        let changes = collect_watch_changes(&slot, &opts, &paths, &cancel).unwrap();
        assert!(apply_watch_batch(
            &mut slot.index.write().unwrap(),
            &mut stats,
            changes
        ));
        let (epoch, revision, len) = slot.fingerprint();
        assert_eq!((stats.dirs, stats.files, len), (1, 6, 8));

        std::fs::write(&paths[0], "changed size").unwrap();
        let changes = collect_watch_changes(&slot, &opts, &paths[..1], &cancel).unwrap();
        assert!(apply_watch_batch(
            &mut slot.index.write().unwrap(),
            &mut stats,
            changes
        ));
        assert_eq!(slot.fingerprint(), (epoch, revision + 1, len));

        // 只报告移入的目录路径时，也能补上它已存在的文件。
        let incoming = dir.path().join("incoming");
        std::fs::create_dir(&incoming).unwrap();
        std::fs::write(incoming.join("new.txt"), "x").unwrap();
        let changes = collect_watch_changes(&slot, &opts, &[incoming], &cancel).unwrap();
        assert!(apply_watch_batch(
            &mut slot.index.write().unwrap(),
            &mut stats,
            changes
        ));
        assert_eq!((stats.dirs, stats.files), (2, 7));
    }

    #[test]
    fn explicit_top_k_is_global_and_puts_unknown_sizes_last() {
        let roots: Vec<_> = [
            (0, Some(3)),
            (1, Some(5)),
            (2, Some(5)),
            (3, None),
            (4, Some(1)),
        ]
        .into_iter()
        .map(|(id, size)| {
            let slot = root(id, PathBuf::from(format!("root-{id}")));
            {
                let mut index = slot.index.write().unwrap();
                let root =
                    index.insert(None, OsStr::new("root"), NodeKind::Dir, NodeMeta::default());
                index.insert(
                    Some(root),
                    OsStr::new("file"),
                    NodeKind::File,
                    NodeMeta {
                        size,
                        ..NodeMeta::default()
                    },
                );
                index.commit();
            }
            slot.work.lock().unwrap().state = RootState::Ready;
            Arc::new(slot)
        })
        .collect();
        let mut spec = QuerySpec {
            text: "file".into(),
            literal: true,
            case_sensitive: false,
            scope: None,
            kind: FileKind::All,
            max_results: 3,
            show_all_on_empty: true,
            sort_column: SortColumn::Size,
            sort_dir: SortDir::Desc,
            home: None,
        };
        let cancel = AtomicBool::new(false);
        let result = run_query(&roots, &spec, &cancel);
        assert_eq!(result.total, 5);
        assert_eq!(
            result.ids.iter().map(|id| id.root().0).collect::<Vec<_>>(),
            [1, 2, 0]
        );
        spec.sort_dir = SortDir::Asc;
        spec.max_results = 0;
        let result = run_query(&roots, &spec, &cancel);
        assert_eq!(
            result.ids.iter().map(|id| id.root().0).collect::<Vec<_>>(),
            [4, 0, 1, 2, 3]
        );
    }
    #[test]
    fn detached_top_k_matches_core_sort_for_every_column_and_direction() {
        let slot = Arc::new(root(0, PathBuf::from("/root")));
        let mut nodes = Vec::new();
        {
            let mut index = slot.index.write().unwrap();
            let root = index.insert(None, OsStr::new("root"), NodeKind::Dir, NodeMeta::default());
            for i in (0..80).rev() {
                nodes.push(index.insert(
                    Some(root),
                    OsStr::new(&format!("file-{:02}", i)),
                    NodeKind::File,
                    NodeMeta {
                        size: (i % 5 != 0).then_some(i % 9),
                        mtime: (i % 3 != 0).then_some(i as i64 - 50),
                        btime: (i % 4 != 0).then_some(100 - i as i64),
                    },
                ));
            }
            index.commit();
        }
        slot.work.lock().unwrap().state = RootState::Ready;
        let mut spec = QuerySpec {
            text: "file".into(),
            literal: true,
            case_sensitive: false,
            scope: None,
            kind: FileKind::All,
            max_results: 7,
            show_all_on_empty: false,
            sort_column: SortColumn::Name,
            sort_dir: SortDir::Asc,
            home: None,
        };
        for column in [
            SortColumn::Name,
            SortColumn::Path,
            SortColumn::Size,
            SortColumn::Modified,
            SortColumn::Created,
        ] {
            for direction in [SortDir::Asc, SortDir::Desc, SortDir::None] {
                spec.sort_column = column;
                spec.sort_dir = direction;
                let expected = {
                    let index = slot.index.read().unwrap();
                    devtools_search_core::query::sort_matches(
                        &index,
                        nodes.clone(),
                        column,
                        direction,
                        7,
                    )
                    .0
                };
                let result = run_query(&[slot.clone()], &spec, &AtomicBool::new(false));
                assert_eq!(result.total, nodes.len());
                assert_eq!(
                    result.ids.iter().map(|id| id.node()).collect::<Vec<_>>(),
                    expected,
                    "{column:?} {direction:?}"
                );
            }
        }
        assert!(run_query(&[slot], &spec, &AtomicBool::new(true)).cancelled);
    }
}
