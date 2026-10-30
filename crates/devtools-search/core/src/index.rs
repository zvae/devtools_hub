//! 根索引：SoA 节点表、共享名字 arena、子节点平面表与 DFS rank（§7.1、§7.9）。
//!
//! - 节点表按列连续存储（parent / name / kind / status / size / mtime / btime），
//!   不给每个文件分配 `Box`/`ThinVec`；
//! - 原始名字、NFC 匹配文本、不区分大小写文本三份常常完全相同（CJK 下
//!   raw == NFC == lower）：共享同一块 arena + 标志位，只在不同时才在附加区存第二/三份；
//! - `revision` 每次提交一批变更就递增（结果集与排序失效，§7.7）；
//!   `index_epoch` 只在重建时递增（旧结果全部失效）；
//! - 节点状态三种（deleted / excluded / pending_remove），同一 epoch 内 NodeId 不复用。

use std::ffi::OsStr;

use serde::{Deserialize, Serialize};

use crate::model::{NameId, NodeId, NodeKind, NodeMeta, NodeStatus, RootId, RootLocation};

/// parent 列的“没有父节点”哨兵（根节点用它）。
pub const PARENT_NONE: u32 = u32::MAX;
/// size 列的 Unknown 哨兵。
const SIZE_UNKNOWN: u64 = u64::MAX;
/// mtime/btime 列的 Unknown 哨兵。
const TIME_UNKNOWN: i64 = i64::MIN;

/// 名字 arena 中的一个条目。
///
/// `nfc`/`lower` 附加区只在文本与已有文本不同时才存（`has_nfc` / `has_lower`），
/// 典型 CJK 主目录下三份文本接近一份（§7.2 预算依据）。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
struct NameEntry {
    raw_off: u32,
    raw_len: u16,
    nfc_off: u32,
    nfc_len: u16,
    lower_off: u32,
    lower_len: u16,
    has_nfc: bool,
    has_lower: bool,
    /// 原始名字是否为合法 UTF-8（非法时没有匹配文本，仍用于显示与路径重建）。
    raw_utf8: bool,
}

/// 名字 arena：原始字节 + 两块按需附加区（NFC / lower）+ 去重索引（不序列化）。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct NameArena {
    raw: Vec<u8>,
    nfc: Vec<u8>,
    lower: Vec<u8>,
    entries: Vec<NameEntry>,
    /// 内容哈希去重索引（开放寻址，容量为 2 的幂，装填系数 ≤ 0.5）；加载后重建。
    #[serde(skip)]
    dedup: Vec<u32>,
}

impl NameArena {
    /// 插入（或复用）一个名字。
    fn intern(&mut self, name: &OsStr) -> NameId {
        let raw_bytes = os_bytes(name);
        let utf8 = std::str::from_utf8(raw_bytes).ok();
        if let Some(existing) = self.lookup(raw_bytes) {
            return NameId(existing as u32);
        }
        let mut entry = NameEntry {
            raw_off: self.raw.len() as u32,
            raw_len: raw_bytes.len().min(u16::MAX as usize) as u16,
            raw_utf8: utf8.is_some(),
            ..NameEntry::default()
        };
        let raw_bytes = &raw_bytes[..entry.raw_len as usize];
        self.raw.extend_from_slice(raw_bytes);
        if let Some(text) = utf8 {
            let nfc = crate::text::nfc(text);
            if nfc.as_bytes() != raw_bytes {
                entry.nfc_off = self.nfc.len() as u32;
                entry.nfc_len = nfc.len() as u16;
                entry.has_nfc = true;
                self.nfc.extend_from_slice(nfc.as_bytes());
            }
            let lower = crate::text::fold(&nfc);
            if lower.as_bytes() != nfc.as_bytes() {
                entry.lower_off = self.lower.len() as u32;
                entry.lower_len = lower.len() as u16;
                entry.has_lower = true;
                self.lower.extend_from_slice(lower.as_bytes());
            }
        }
        self.entries.push(entry);
        let slot = self.entries.len() as u32 - 1;
        self.index_insert(raw_bytes, slot);
        NameId(slot)
    }

    fn entry(&self, id: NameId) -> &NameEntry {
        &self.entries[id.0 as usize]
    }

    fn raw_slice(&self, entry: &NameEntry) -> &[u8] {
        let start = entry.raw_off as usize;
        &self.raw[start..start + entry.raw_len as usize]
    }

    fn nfc_slice(&self, entry: &NameEntry) -> &[u8] {
        if entry.has_nfc {
            let start = entry.nfc_off as usize;
            &self.nfc[start..start + entry.nfc_len as usize]
        } else {
            self.raw_slice(entry)
        }
    }

    fn lower_slice(&self, entry: &NameEntry) -> &[u8] {
        if entry.has_lower {
            let start = entry.lower_off as usize;
            &self.lower[start..start + entry.lower_len as usize]
        } else {
            self.nfc_slice(entry)
        }
    }

    fn index_insert(&mut self, bytes: &[u8], slot: u32) {
        if self.dedup.len() < self.entries.len() * 2 {
            self.rebuild_index();
        }
        let mask = self.dedup.len() - 1;
        let mut probe = hash_bytes(bytes) as usize & mask;
        while self.dedup[probe] != u32::MAX {
            probe = (probe + 1) & mask;
        }
        self.dedup[probe] = slot;
    }

    fn lookup(&self, bytes: &[u8]) -> Option<usize> {
        if self.dedup.is_empty() {
            return None;
        }
        let mask = self.dedup.len() - 1;
        let mut probe = hash_bytes(bytes) as usize & mask;
        loop {
            let stored = self.dedup[probe];
            if stored == u32::MAX {
                return None;
            }
            if self.raw_slice(&self.entries[stored as usize]) == bytes {
                return Some(stored as usize);
            }
            probe = (probe + 1) & mask;
        }
    }

    /// 扩容并重放全部条目（调用时 dedup 为空或容量不足）。
    fn rebuild_index(&mut self) {
        let mut capacity = 64;
        while capacity < self.entries.len() * 2 {
            capacity *= 2;
        }
        self.dedup = vec![u32::MAX; capacity];
        for slot in 0..self.entries.len() {
            let bytes = self.raw_slice(&self.entries[slot]).to_vec();
            let mask = self.dedup.len() - 1;
            let mut probe = hash_bytes(&bytes) as usize & mask;
            while self.dedup[probe] != u32::MAX {
                probe = (probe + 1) & mask;
            }
            self.dedup[probe] = slot as u32;
        }
    }

    /// 反序列化后重建去重索引。
    fn ensure_index(&mut self) {
        if self.dedup.is_empty() && !self.entries.is_empty() {
            self.rebuild_index();
        }
    }
}

/// 简单 FNV-1a 哈希。
fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// 平铺的列存储（快照直接序列化这些列，§7.6）。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct Columns {
    parent: Vec<u32>,
    name: Vec<u32>,
    kind: Vec<u8>,
    status: Vec<u8>,
    size: Vec<u64>,
    mtime: Vec<i64>,
    btime: Vec<i64>,
}

/// 子节点平面表与 DFS 派生数据（§7.9 默认路径序、§7.8 infolder 的欧拉区间）。
///
/// 结构变更后由 [`RootIndex::rebuild_derived`] 重建；rank 与结构绑定。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct Derived {
    /// 每个节点的孩子区间（child_list 的下标）。
    child_range: Vec<(u32, u32)>,
    /// 按父节点分桶、桶内按名字顺序的孩子列表。
    child_list: Vec<u32>,
    /// DFS 先序序号（默认路径序；NodeId 顺序绝不能作路径顺序，§7.9）。
    rank: Vec<u32>,
    /// 欧拉进入时间。
    tin: Vec<u32>,
    /// 欧拉离开时间（区间 [tin, tout) 覆盖全部后代）。
    tout: Vec<u32>,
    /// 派生数据对应的节点数量（与列长度不一致时失效）。
    built_for: usize,
}

/// 一个根的索引。
#[derive(Debug, Default)]
pub struct RootIndex {
    root: RootId,
    identity: Option<RootLocation>,
    epoch: u32,
    revision: u64,
    columns: Columns,
    names: NameArena,
    derived: Derived,
    next_node: u32,
    deleted: u32,
    excluded: u32,
}

impl RootIndex {
    /// 新建空索引（扫描前）。
    pub fn new(root: RootId) -> Self {
        Self {
            root,
            ..Self::default()
        }
    }

    pub fn root(&self) -> RootId {
        self.root
    }

    pub fn identity(&self) -> Option<&RootLocation> {
        self.identity.as_ref()
    }

    pub fn set_identity(&mut self, identity: RootLocation) {
        self.identity = Some(identity);
    }

    /// 代际（重建/压缩后递增；旧结果全部失效，§7.1）。
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// 修订号（每次提交一批变更递增；结果集与排序失效，§7.7）。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn len(&self) -> u32 {
        self.next_node
    }

    pub fn is_empty(&self) -> bool {
        self.next_node == 0
    }

    pub fn node_id_valid(&self, node: NodeId) -> bool {
        node.0 < self.next_node
    }

    pub fn kind(&self, node: NodeId) -> NodeKind {
        NodeKind::from_u8(self.columns.kind[node.as_usize()])
    }

    pub fn status(&self, node: NodeId) -> NodeStatus {
        NodeStatus::from_u8(self.columns.status[node.as_usize()])
    }

    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        let p = self.columns.parent[node.as_usize()];
        (p != PARENT_NONE).then(|| NodeId(p))
    }

    pub fn name_id(&self, node: NodeId) -> NameId {
        NameId(self.columns.name[node.as_usize()])
    }

    /// 原始名字字节（Unix 无损；Windows 为 lossy 表示）。
    pub fn raw_name(&self, node: NodeId) -> &[u8] {
        self.names.raw_slice(self.names.entry(self.name_id(node)))
    }

    /// 原始名字字符串（非法 UTF-8 时 `None`）。
    pub fn raw_name_str(&self, node: NodeId) -> Option<&str> {
        let entry = self.names.entry(self.name_id(node));
        if !entry.raw_utf8 {
            return None;
        }
        std::str::from_utf8(self.names.raw_slice(entry)).ok()
    }

    /// 匹配文本：区分大小写 → NFC；不区分 → lower（§7.1）。
    /// 原始名字不是合法 UTF-8 时返回 `None`（该节点无法匹配文本词）。
    pub fn match_text(&self, node: NodeId, case_sensitive: bool) -> Option<&str> {
        let entry = self.names.entry(self.name_id(node));
        if !entry.raw_utf8 {
            return None;
        }
        let bytes = if case_sensitive {
            self.names.nfc_slice(entry)
        } else {
            self.names.lower_slice(entry)
        };
        std::str::from_utf8(bytes).ok()
    }

    /// 大小写不敏感比较用的排序键文本。
    pub fn sort_name(&self, node: NodeId) -> &str {
        self.match_text(node, false).unwrap_or_default()
    }

    pub fn meta(&self, node: NodeId) -> NodeMeta {
        let i = node.as_usize();
        NodeMeta {
            size: (self.columns.size[i] != SIZE_UNKNOWN).then_some(self.columns.size[i]),
            mtime: (self.columns.mtime[i] != TIME_UNKNOWN).then_some(self.columns.mtime[i]),
            btime: (self.columns.btime[i] != TIME_UNKNOWN).then_some(self.columns.btime[i]),
        }
    }

    /// 默认路径序（rank）。派生数据失效时调用方应先 [`Self::rebuild_derived`]。
    pub fn rank_of(&self, node: NodeId) -> u32 {
        if self.derived.built_for == self.columns.parent.len() {
            self.derived.rank[node.as_usize()]
        } else {
            node.0
        }
    }

    /// `node` 是否是 `anc` 的后代（含自身；基于 DFS 欧拉区间，§7.8）。
    /// 派生数据失效时返回 `false`。
    pub fn is_descendant(&self, anc: NodeId, node: NodeId) -> bool {
        if self.derived.built_for != self.columns.parent.len() {
            return false;
        }
        let tin_a = self.derived.tin[anc.as_usize()];
        let tout_a = self.derived.tout[anc.as_usize()];
        let tin_n = self.derived.tin[node.as_usize()];
        tin_a <= tin_n && tin_n < tout_a
    }

    /// 派生数据是否与当前结构一致（rank 可用）。
    pub fn derived_valid(&self) -> bool {
        self.derived.built_for == self.columns.parent.len()
    }

    /// 孩子列表（桶内已按名字不区分大小写顺序）。
    pub fn children(&self, node: NodeId) -> &[NodeId] {
        if !self.derived_valid() {
            return &[];
        }
        let (start, end) = self.derived.child_range[node.as_usize()];
        // SAFETY：NodeId 是 `#[repr(transparent)]` 等价的 u32 包装；这里用指针转换得到
        // `&[NodeId]` 视图，避免百万级节点的转换分配。
        let slice = &self.derived.child_list[start as usize..end as usize];
        unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const NodeId, slice.len()) }
    }

    // ---- 变更操作（扫描与事件应用；小批提交，§7.7） ----

    /// 插入节点（不触发派生重建；批量插入后调用 [`Self::commit`]）。
    pub fn insert(
        &mut self,
        parent: Option<NodeId>,
        name: &OsStr,
        kind: NodeKind,
        meta: NodeMeta,
    ) -> NodeId {
        let id = self.next_node;
        debug_assert_eq!(
            self.columns.parent.len(),
            id as usize,
            "列长度必须与节点数一致"
        );
        self.next_node += 1;
        self.columns.parent.push(parent.map_or(PARENT_NONE, |p| p.0));
        self.columns.name.push(self.names.intern(name).0);
        self.columns.kind.push(kind as u8);
        self.columns.status.push(NodeStatus::Live as u8);
        self.columns.size.push(meta.size.unwrap_or(SIZE_UNKNOWN));
        self.columns.mtime.push(meta.mtime.unwrap_or(TIME_UNKNOWN));
        self.columns.btime.push(meta.btime.unwrap_or(TIME_UNKNOWN));
        NodeId(id)
    }

    /// 标记删除（同一 epoch 内 NodeId 不复用，§7.1）。
    pub fn mark_deleted(&mut self, node: NodeId) {
        let i = node.as_usize();
        if self.columns.status[i] == NodeStatus::Live as u8 {
            self.columns.status[i] = NodeStatus::Deleted as u8;
            self.deleted += 1;
        }
    }

    /// 批量删除中的临时隐藏（提交后转为 deleted，§7.1）。
    pub fn mark_pending_remove(&mut self, node: NodeId) {
        let i = node.as_usize();
        if self.columns.status[i] == NodeStatus::Live as u8 {
            self.columns.status[i] = NodeStatus::PendingRemove as u8;
        }
    }

    /// pending_remove → deleted（批量删除提交后的转换）。
    pub fn flush_pending_remove(&mut self) {
        for i in 0..self.columns.status.len() {
            if self.columns.status[i] == NodeStatus::PendingRemove as u8 {
                self.columns.status[i] = NodeStatus::Deleted as u8;
                self.deleted += 1;
            }
        }
    }

    /// 应用/解除排除掩码（可恢复，不等于删除节点，§6.3、§7.1）。
    pub fn set_excluded(&mut self, node: NodeId, excluded: bool) {
        let i = node.as_usize();
        let current = NodeStatus::from_u8(self.columns.status[i]);
        let next = match (current, excluded) {
            (NodeStatus::Live, true) => {
                self.excluded += 1;
                NodeStatus::Excluded
            }
            (NodeStatus::Excluded, false) => {
                self.excluded = self.excluded.saturating_sub(1);
                NodeStatus::Live
            }
            _ => return,
        };
        self.columns.status[i] = next as u8;
    }

    /// 提交一批变更：递增 revision、重建派生数据。
    pub fn commit(&mut self) {
        self.revision += 1;
        self.rebuild_derived();
    }

    /// 重建子节点表、rank 与欧拉区间（孩子按名字不区分大小写顺序，§7.9）。
    pub fn rebuild_derived(&mut self) {
        let n = self.columns.parent.len();
        let mut derived = Derived {
            child_range: vec![(0, 0); n],
            child_list: Vec::new(),
            rank: vec![0; n],
            tin: vec![0; n],
            tout: vec![u32::MAX; n],
            built_for: n,
        };
        if n == 0 {
            self.derived = derived;
            return;
        }
        // 计数排序按父节点分桶。
        let mut counts = vec![0u32; n];
        let mut root = u32::MAX;
        for (id, &p) in self.columns.parent.iter().enumerate() {
            if p == PARENT_NONE {
                root = id as u32;
            } else {
                counts[p as usize] += 1;
            }
        }
        let mut starts = vec![0u32; n];
        let mut acc = 0u32;
        for v in 0..n {
            starts[v] = acc;
            acc += counts[v];
        }
        let mut child_list = vec![0u32; acc as usize];
        let mut cursor = starts.clone();
        for id in 0..n as u32 {
            let p = self.columns.parent[id as usize];
            if p != PARENT_NONE {
                let slot = cursor[p as usize] as usize;
                child_list[slot] = id;
                cursor[p as usize] += 1;
            }
        }
        // 桶内按名字（不区分大小写）稳定排序。
        for v in 0..n {
            let start = starts[v] as usize;
            let end = start + counts[v] as usize;
            if end - start > 1 {
                child_list[start..end].sort_by(|&a, &b| {
                    self.sort_name(NodeId(a))
                        .cmp(self.sort_name(NodeId(b)))
                        .then_with(|| a.cmp(&b))
                });
            }
            derived.child_range[v] = (starts[v], starts[v] + counts[v]);
        }
        // 从根 DFS（先序），生成 rank 与 tin/tout。
        if root != u32::MAX {
            let mut cursors = starts.clone();
            let mut stack = vec![root];
            let mut tin = 0u32;
            let mut rank = 0u32;
            derived.tin[root as usize] = tin;
            tin += 1;
            derived.rank[root as usize] = rank;
            rank += 1;
            while let Some(&top) = stack.last() {
                let (_, end) = derived.child_range[top as usize];
                if cursors[top as usize] < end {
                    let child = child_list[cursors[top as usize] as usize];
                    cursors[top as usize] += 1;
                    derived.tin[child as usize] = tin;
                    tin += 1;
                    derived.rank[child as usize] = rank;
                    rank += 1;
                    stack.push(child);
                } else {
                    derived.tout[top as usize] = tin;
                    stack.pop();
                }
            }
        }
        derived.child_list = child_list;
        self.derived = derived;
    }

    /// 墓碑/排除占比（压缩启发式，§7.7：超过约 20% 时后台压缩）。
    pub fn stale_ratio(&self) -> f64 {
        let total = self.next_node.max(1);
        f64::from(self.deleted + self.excluded) / f64::from(total)
    }

    /// 重建索引（旁路建新根后替换；分配新 epoch，旧结果全部失效，§7.3）。
    pub fn finish_rebuild(&mut self) {
        self.epoch += 1;
        self.deleted = 0;
        self.excluded = self
            .columns
            .status
            .iter()
            .filter(|&&s| s == NodeStatus::Excluded as u8)
            .count() as u32;
        self.names.ensure_index();
        self.rebuild_derived();
    }

    pub(crate) fn parts(&self) -> (&Columns, &NameArena, u32) {
        (&self.columns, &self.names, self.next_node)
    }

    pub(crate) fn from_parts(
        columns: Columns,
        mut names: NameArena,
        next_node: u32,
        epoch: u32,
        revision: u64,
    ) -> Self {
        let mut index = Self::default();
        index.epoch = epoch;
        index.revision = revision;
        index.next_node = next_node;
        index.deleted = columns
            .status
            .iter()
            .filter(|&&s| s == NodeStatus::Deleted as u8)
            .count() as u32;
        index.excluded = columns
            .status
            .iter()
            .filter(|&&s| s == NodeStatus::Excluded as u8)
            .count() as u32;
        names.ensure_index();
        index.names = names;
        index.columns = columns;
        index.rebuild_derived();
        index
    }

    /// 根节点（parent 为哨兵的那个；空索引为 `None`）。
    pub fn root_node(&self) -> Option<NodeId> {
        (0..self.next_node)
            .map(NodeId)
            .find(|&n| self.parent(n).is_none())
    }
}

fn os_bytes(name: &OsStr) -> &[u8] {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        name.as_bytes()
    }
    #[cfg(not(unix))]
    {
        // Windows：OsStr 的 WTF-8 表示没有公开的字节接口；
        // 用 to_string_lossy 的字节（非法 UTF-8 的罕见情形由 raw_utf8 标记）。
        name.to_string_lossy().as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(name: &str) -> &OsStr {
        OsStr::new(name)
    }

    /// 构造一个小树：
    /// ```text
    /// / (root "root")
    /// ├── b.txt
    /// ├── docs (dir)
    /// │   ├── a.md
    /// │   └── B.md
    /// └── a (dir)
    ///     └── c.log
    /// ```
    fn sample() -> RootIndex {
        let mut index = RootIndex::new(RootId(0));
        let root = index.insert(None, os("root"), NodeKind::Dir, NodeMeta::default());
        index.insert(Some(root), os("b.txt"), NodeKind::File, NodeMeta { size: Some(10), ..Default::default() });
        let docs = index.insert(Some(root), os("docs"), NodeKind::Dir, NodeMeta::default());
        index.insert(Some(docs), os("a.md"), NodeKind::File, NodeMeta::default());
        index.insert(Some(docs), os("B.md"), NodeKind::File, NodeMeta::default());
        let a = index.insert(Some(root), os("a"), NodeKind::Dir, NodeMeta::default());
        index.insert(Some(a), os("c.log"), NodeKind::File, NodeMeta::default());
        index.commit();
        index
    }

    #[test]
    fn rank_is_dfs_name_order_not_node_id_order() {
        let index = sample();
        let root = index.root_node().unwrap();
        // 孩子按名字顺序：a、b.txt、docs。
        let names: Vec<&str> = index
            .children(root)
            .iter()
            .filter_map(|&c| index.raw_name_str(c))
            .collect();
        assert_eq!(names, ["a", "b.txt", "docs"]);
        // rank：root=0, a=1, c.log=2, b.txt=3, docs=4, a.md=5, B.md=6。
        let find = |name: &str| (0..index.len()).map(NodeId).find(|&n| index.raw_name_str(n) == Some(name)).unwrap();
        let expected = [("root", 0u32), ("a", 1), ("c.log", 2), ("b.txt", 3), ("docs", 4), ("a.md", 5), ("B.md", 6)];
        for (name, rank) in expected {
            assert_eq!(index.rank_of(find(name)), rank, "{name} 的 rank");
        }
    }

    #[test]
    fn descendant_test_via_euler_intervals() {
        let index = sample();
        let find = |name: &str| (0..index.len()).map(NodeId).find(|&n| index.raw_name_str(n) == Some(name)).unwrap();
        let docs = find("docs");
        let a_md = find("a.md");
        let b_txt = find("b.txt");
        assert!(index.is_descendant(docs, a_md));
        assert!(index.is_descendant(docs, docs));
        assert!(!index.is_descendant(docs, b_txt));
    }

    #[test]
    fn node_statuses_are_independent() {
        let mut index = sample();
        let b = index.root_node().unwrap();
        let b_txt = index.children(b).iter().find(|n| index.raw_name_str(**n) == Some("b.txt")).copied().unwrap();
        // 排除：可恢复，ID 不变。
        index.set_excluded(b_txt, true);
        assert_eq!(index.status(b_txt), NodeStatus::Excluded);
        assert!(!index.status(b_txt).visible());
        index.set_excluded(b_txt, false);
        assert_eq!(index.status(b_txt), NodeStatus::Live);
        // 删除：ID 不复用（next_node 不回退）。
        let before = index.len();
        index.mark_deleted(b_txt);
        assert_eq!(index.status(b_txt), NodeStatus::Deleted);
        assert_eq!(index.len(), before);
        assert!(index.stale_ratio() > 0.0);
        // pending_remove → deleted。
        let a_md = (0..index.len()).map(NodeId).find(|&n| index.raw_name_str(n) == Some("a.md")).unwrap();
        index.mark_pending_remove(a_md);
        assert_eq!(index.status(a_md), NodeStatus::PendingRemove);
        index.flush_pending_remove();
        assert_eq!(index.status(a_md), NodeStatus::Deleted);
    }

    #[test]
    fn match_text_uses_shared_arena() {
        let mut index = RootIndex::new(RootId(0));
        let root = index.insert(None, os("根目录"), NodeKind::Dir, NodeMeta::default());
        let nfc_name = index.insert(Some(root), os("已NFC"), NodeKind::File, NodeMeta::default());
        let mixed = index.insert(Some(root), os("MiXeD.CsV"), NodeKind::File, NodeMeta::default());
        index.commit();
        // CJK：raw == nfc == lower，只有 raw 一份。
        assert_eq!(index.match_text(nfc_name, true), Some("已NFC"));
        assert_eq!(index.match_text(nfc_name, false), Some("已nfc"));
        assert_eq!(index.names.entries.len(), 3);
        assert!(!index.names.entry(index.name_id(nfc_name)).has_nfc);
        assert!(index.names.entry(index.name_id(mixed)).has_lower);
        // 大小写不敏感文本 = lower。
        assert_eq!(index.match_text(mixed, false), Some("mixed.csv"));
        assert_eq!(index.match_text(mixed, true), Some("MiXeD.CsV"));
    }

    #[test]
    fn same_name_shares_one_arena_entry() {
        let mut index = RootIndex::new(RootId(0));
        let root = index.insert(None, os("r"), NodeKind::Dir, NodeMeta::default());
        index.insert(Some(root), os("dup.txt"), NodeKind::File, NodeMeta::default());
        index.insert(Some(root), os("dup.txt"), NodeKind::File, NodeMeta::default());
        index.commit();
        assert_eq!(index.names.entries.len(), 2);
    }
}
