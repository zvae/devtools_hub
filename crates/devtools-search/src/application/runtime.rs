//! 运行时：专用查询/扫描线程池、各根 live 索引、快照载入与取消（§7.3、§7.9、§7.11）。
//!
//! 线程模型：Service 在 UI 线程串行处理命令；扫描与查询跑在专用 rayon 池里，结果通过
//! 事件通道回传。live 索引用 `RwLock` 保护，扫描完成时整体替换（epoch+1），查询只在读锁下工作。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use devtools_search_core::query::{self, Ast, EvalOptions, Term};
use devtools_search_core::scan::{self, ExcludeSet, ScanOptions, ScanStats};
use devtools_search_core::snapshot::{config_fingerprint, EventCheckpoint, SnapshotStore};
use devtools_search_core::volumes::RootState;
use devtools_search_core::{
    IndexConfig, NodeId, NodeKind, ResultId, RootId, RootIndex, RootLocation, SortColumn, SortDir,
};

use super::protocol::{Event, RootStatus, RowData, SearchResult};

/// 一个根的内存槽位。
pub struct RootSlot {
    pub id: RootId,
    pub identity: RootLocation,
    /// 根对应的绝对路径（挂载点 + 相对路径）。
    pub mount: PathBuf,
    pub index: RwLock<RootIndex>,
    pub state: Mutex<RootState>,
    pub scan_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub stats: Mutex<ScanStats>,
    pub checkpoint: Mutex<EventCheckpoint>,
    pub from_snapshot: AtomicBool,
}

impl RootSlot {
    fn new(id: RootId, identity: RootLocation, mount: PathBuf) -> Self {
        Self {
            id,
            identity,
            mount,
            index: RwLock::new(RootIndex::new(id)),
            state: Mutex::new(RootState::Dormant),
            scan_cancel: Mutex::new(None),
            stats: Mutex::new(ScanStats::default()),
            checkpoint: Mutex::new(EventCheckpoint::default()),
            from_snapshot: AtomicBool::new(false),
        }
    }

    pub fn state(&self) -> RootState {
        self.state.lock().unwrap().clone()
    }

    fn set_state(&self, state: RootState) {
        *self.state.lock().unwrap() = state;
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
    pub max_results: u32,
    pub show_all_on_empty: bool,
    pub sort_column: SortColumn,
    pub sort_dir: SortDir,
    pub home: Option<PathBuf>,
}

/// 运行时。
pub struct Runtime {
    query_pool: rayon::ThreadPool,
    scan_pool: rayon::ThreadPool,
    snapshot_dir: PathBuf,
    slots: RwLock<BTreeMap<RootId, Arc<RootSlot>>>,
    events: Mutex<Sender<Event>>,
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
        Self {
            query_pool,
            scan_pool,
            snapshot_dir,
            slots: RwLock::new(BTreeMap::new()),
            events: Mutex::new(events),
            repaint,
            next_id: AtomicU16::new(0),
            generation: AtomicU64::new(0),
        }
    }

    fn sender(&self) -> Option<Sender<Event>> {
        self.events.lock().ok().map(|tx| tx.clone())
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
        self.slots.write().unwrap().remove(&id)
    }

    /// 全部根的状态（顺序稳定按 RootId）。
    pub fn roots_status(&self) -> Vec<RootStatus> {
        self.slots()
            .into_iter()
            .map(|slot| status_of(&slot))
            .collect()
    }

    /// 开始扫描一个根：先尝试载入快照（立即可查），再全量扫描替换（§7.5 PendingValidation）。
    pub fn spawn_scan(
        &self,
        slot: Arc<RootSlot>,
        config: IndexConfig,
        persist: bool,
    ) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        // 同一根同一时间只允许一个扫描任务。
        if let Some(previous) = slot.scan_cancel.lock().unwrap().replace(cancel.clone()) {
            previous.store(true, Ordering::SeqCst);
        }
        let fingerprint = config_fingerprint(&config);
        let excludes = ExcludeSet::compile(&config.exclude_rules, config.exclude_case_sensitive)
            .unwrap_or_else(|_| ExcludeSet::compile(&[], false).expect("空规则总是可编译"));
        let key = devtools_search_core::root_key(&slot.identity);
        let tx = self.sender();
        let repaint = self.repaint.clone();
        let snapshot_dir = self.snapshot_dir.clone();
        let scan_app_bundles = config.scan_app_bundles;
        self.scan_pool.spawn({
            let slot = slot.clone();
            let cancel = cancel.clone();
            move || {
                let emit_roots = |slot: &RootSlot| {
                    if let Some(tx) = &tx {
                        let _ = tx.send(Event::Roots { roots: vec![status_of(slot)] });
                    }
                    repaint();
                };
                // 1) 快照：立即给出可查数据。
                slot.set_state(RootState::PendingValidation);
                emit_roots(&slot);
                if persist {
                    let store = SnapshotStore::new(snapshot_dir.clone());
                    match store.load_latest(&key) {
                        Ok(Some(snapshot))
                            if snapshot.identity == slot.identity
                                && snapshot.config_fingerprint == fingerprint =>
                        {
                            {
                                let mut index = slot.index.write().unwrap();
                                *index = snapshot.index;
                            }
                            *slot.checkpoint.lock().unwrap() = snapshot.checkpoint;
                            slot.from_snapshot.store(true, Ordering::Relaxed);
                            {
                                let mut stats = slot.stats.lock().unwrap();
                                stats.files = slot.len();
                                stats.dirs = 0;
                            }
                            slot.set_state(RootState::Ready);
                            emit_roots(&slot);
                        }
                        Ok(_) => {}
                        Err(err) => {
                            tracing::warn!("载入快照失败（{}）：{err}", slot.mount.display())
                        }
                    }
                }
                if cancel.load(Ordering::SeqCst) {
                    slot.set_state(RootState::Paused);
                    *slot.scan_cancel.lock().unwrap() = None;
                    emit_roots(&slot);
                    return;
                }
                // 2) 全量扫描到新索引（旁路构建，完成后整体替换）。
                slot.set_state(RootState::Scanning);
                emit_roots(&slot);
                let mut fresh = RootIndex::new(slot.id);
                fresh.set_identity(slot.identity.clone());
                let opts = ScanOptions {
                    excludes,
                    scan_app_bundles,
                    root_device: device_of(&slot.mount),
                };
                let outcome = scan::scan_root(
                    &crate::platform::fs::StdFs,
                    &slot.mount,
                    &opts,
                    &cancel,
                    &mut fresh,
                );
                if outcome.cancelled || cancel.load(Ordering::SeqCst) {
                    slot.set_state(RootState::Paused);
                    *slot.scan_cancel.lock().unwrap() = None;
                    emit_roots(&slot);
                    return;
                }
                *slot.stats.lock().unwrap() = outcome.stats;
                slot.from_snapshot.store(false, Ordering::Relaxed);
                {
                    // 整体替换：分配新 epoch（§7.3：旧结果全部失效）。
                    fresh.finish_rebuild();
                    let mut index = slot.index.write().unwrap();
                    *index = fresh;
                }
                slot.set_state(RootState::Ready);
                *slot.scan_cancel.lock().unwrap() = None;
                // 3) 保存快照。
                if persist {
                    let store = SnapshotStore::new(snapshot_dir.clone());
                    let index = slot.index.read().unwrap();
                    let checkpoint = slot.checkpoint.lock().unwrap().clone();
                    if let Err(err) =
                        store.save(&key, &index, &slot.identity, fingerprint, &checkpoint)
                    {
                        tracing::warn!("保存快照失败：{err}");
                    }
                }
                emit_roots(&slot);
            }
        });
        cancel
    }

    /// 取消一个根的扫描。
    pub fn cancel_scan(&self, slot: &RootSlot) {
        if let Some(cancel) = slot.scan_cancel.lock().unwrap().take() {
            cancel.store(true, Ordering::SeqCst);
        }
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
        let tx = self.sender();
        let repaint = self.repaint.clone();
        self.query_pool.spawn(move || {
            let started = Instant::now();
            let outcome = run_query(&slots, &spec, &cancel);
            if outcome.cancelled || cancel.load(Ordering::SeqCst) {
                return;
            }
            if let Some(tx) = &tx {
                let _ = tx.send(Event::SearchDone(SearchResult {
                    session,
                    generation,
                    total: outcome.total,
                    took_ms: started.elapsed().as_millis() as u64,
                    error: outcome.error,
                    notice: outcome.notice,
                    ids: outcome.ids,
                }));
            }
            repaint();
        });
    }

    /// 读取一屏行数据。
    pub fn load_rows(&self, ids: &[ResultId], start: u32, count: u32) -> Vec<RowData> {
        let end = start.saturating_add(count).min(ids.len() as u32);
        let mut rows = Vec::with_capacity(end.saturating_sub(start) as usize);
        for index in start..end {
            let id = ids[index as usize];
            let Some(slot) = self.slot(id.root()) else {
                rows.push(missing_row(id, index));
                continue;
            };
            let guard = slot.index.read().unwrap();
            rows.push(build_row(&slot, &guard, id, index));
        }
        rows
    }

    /// 关闭：取消全部扫描任务。线程池在 Service 释放时 drop（有界阻塞）。
    pub fn shutdown(&self) {
        for slot in self.slots() {
            if let Some(cancel) = slot.scan_cancel.lock().unwrap().take() {
                cancel.store(true, Ordering::SeqCst);
            }
        }
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

fn run_query(slots: &[Arc<RootSlot>], spec: &QuerySpec, cancel: &AtomicBool) -> QueryOutcome {
    // 空查询：只有"列出全部"或指定了目录范围时才继续（范围本身就是过滤条件）。
    if spec.text.trim().is_empty() && !spec.show_all_on_empty && spec.scope.is_none() {
        return QueryOutcome::empty(None, None);
    }
    // 解析（字面模式跳过语法，§4.5）。
    let ast = if spec.text.trim().is_empty() {
        Ast::And(Vec::new())
    } else if spec.literal {
        Ast::Term(Term::Name {
            pattern: spec.text.clone(),
            wildcards: false,
        })
    } else {
        match query::parse(&spec.text) {
            Ok(ast) => ast,
            Err(err) => return QueryOutcome::empty(Some(err.to_string()), None),
        }
    };
    // 目录范围：入口带选中的文件夹时作为 infolder 过滤（§4.5）。
    let ast = match &spec.scope {
        Some(scope) => Ast::And(vec![
            ast,
            Ast::Term(Term::InFolder(scope.to_string_lossy().into_owned())),
        ]),
        None => ast,
    };
    let mut per_root: Vec<(usize, Vec<NodeId>)> = Vec::new();
    let mut total = 0usize;
    let mut notice: Option<String> = None;
    for (slot_index, slot) in slots.iter().enumerate() {
        if matches!(slot.state(), RootState::Offline | RootState::Dormant) {
            continue;
        }
        let index = slot.index.read().unwrap();
        if index.is_empty() {
            continue;
        }
        let opts = EvalOptions {
            case_sensitive: spec.case_sensitive,
            home: spec.home.clone(),
            root_abs: slot.mount.clone(),
        };
        let outcome = query::eval(&index, &ast, &opts, cancel);
        if outcome.cancelled {
            return QueryOutcome::cancelled();
        }
        if let Some(query::Notice::NotIndexed { value }) = outcome.notice {
            notice.get_or_insert(format!("{value} 不在索引范围内，可把它添加为索引根"));
        }
        total += outcome.matches.len();
        per_root.push((slot_index, outcome.matches));
    }
    let limit = if spec.max_results == 0 {
        usize::MAX
    } else {
        spec.max_results as usize
    };
    let mut ids: Vec<ResultId> = Vec::with_capacity(total.min(limit));
    match (spec.sort_column, spec.sort_dir) {
        (SortColumn::Path, _) | (_, SortDir::None) => {
            // 默认路径序：根按挂载路径稳定排序，根内按 rank（§7.9）。
            let mut roots = per_root.clone();
            roots.sort_by(|a, b| slots[a.0].mount.cmp(&slots[b.0].mount));
            let mut remaining = limit;
            'outer: for (slot_index, matches) in roots {
                let slot = &slots[slot_index];
                let index = slot.index.read().unwrap();
                let (sorted, _) =
                    query::sort_matches(&index, matches, SortColumn::Path, SortDir::None, remaining);
                for node in sorted {
                    ids.push(ResultId::new(slot.id, node));
                    remaining = remaining.saturating_sub(1);
                    if remaining == 0 {
                        break 'outer;
                    }
                }
            }
        }
        (column, dir) => {
            // 显式排序：跨根全局排序（同时持全部读锁，保证键读取一致）。
            let guards: Vec<_> = slots.iter().map(|s| s.index.read().unwrap()).collect();
            let mut all: Vec<(usize, NodeId)> = Vec::new();
            for (slot_index, matches) in &per_root {
                for &node in matches {
                    all.push((*slot_index, node));
                }
            }
            all.sort_by(|&(sa, na), &(sb, nb)| {
                compare_across(&guards[sa], na, &guards[sb], nb, column, dir)
            });
            all.truncate(limit);
            for (slot_index, node) in all {
                ids.push(ResultId::new(slots[slot_index].id, node));
            }
        }
    }
    QueryOutcome {
        ids,
        total,
        error: None,
        notice,
        cancelled: false,
    }
}

/// 跨根比较：未知元数据置后（与方向无关）；相同键按路径与 rank 保证确定性。
fn compare_across(
    a: &RootIndex,
    na: NodeId,
    b: &RootIndex,
    nb: NodeId,
    column: SortColumn,
    dir: SortDir,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (ma, mb) = (a.meta(na), b.meta(nb));
    let (miss_a, miss_b) = match column {
        SortColumn::Size => (ma.size.is_none(), mb.size.is_none()),
        SortColumn::Modified => (ma.mtime.is_none(), mb.mtime.is_none()),
        SortColumn::Created => (ma.btime.is_none(), mb.btime.is_none()),
        _ => (false, false),
    };
    let missing = miss_a.cmp(&miss_b);
    if missing != Ordering::Equal {
        return missing;
    }
    let ord = match column {
        SortColumn::Name => a.sort_name(na).cmp(b.sort_name(nb)),
        SortColumn::Size => ma.size.unwrap_or(0).cmp(&mb.size.unwrap_or(0)),
        SortColumn::Modified => ma.mtime.unwrap_or(0).cmp(&mb.mtime.unwrap_or(0)),
        SortColumn::Created => ma.btime.unwrap_or(0).cmp(&mb.btime.unwrap_or(0)),
        SortColumn::Path => Ordering::Equal,
    };
    let ord = if dir == SortDir::Desc {
        ord.reverse()
    } else {
        ord
    };
    ord.then_with(|| a.rank_of(na).cmp(&b.rank_of(nb)))
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
    if !index.node_id_valid(node) || !index.status(node).visible() {
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
                        .unwrap_or_else(|| String::from_utf8_lossy(index.raw_name(cur)).into_owned()),
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
    let stats = slot.stats.lock().unwrap();
    RootStatus {
        id: slot.id,
        path: slot.mount.clone(),
        state: slot.state(),
        files: stats.files,
        dirs: stats.dirs,
        from_snapshot: slot.from_snapshot.load(Ordering::Relaxed),
    }
}

fn device_of(path: &std::path::Path) -> Option<u64> {
    use devtools_search_core::ports::FsSource;
    crate::platform::fs::StdFs.device_of(path)
}
