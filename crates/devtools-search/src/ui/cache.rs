//! 可见行缓存（§7.10 LoadRows：128 行一批）。
//!
//! 结果集变化时按 `ResultId` 增量重排：仍在新结果里的行只改下标、内容保留，
//! 只有真正新增/消失的行才留下空洞。索引更新期间重跑查询不会把整张表清空再重填，
//! 界面也就不会整屏闪一下。缓存有上限，滚动长时间浏览不会无限增长。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use devtools_search_core::{ResultId, RootId};

use crate::application::protocol::RowData;

/// 一批的行数。
pub const CHUNK: u32 = 128;
/// 缓存行上限，优先保留靠近当前视野的批次。
const MAX_ROWS: usize = 4096;

#[derive(Default)]
pub struct RowCache {
    rows: BTreeMap<u32, RowData>,
    /// 已请求（或已完整加载）的批号。
    chunks: HashSet<u32>,
    /// 当前结果集的顺序：增量重排的依据，也用来丢弃迟到的旧响应。
    ids: Arc<Vec<ResultId>>,
}

impl RowCache {
    pub fn clear(&mut self) {
        self.release_rows();
        self.ids = Arc::default();
    }

    /// 关窗后回收可见行，保留结果 ID 供下次打开时按需取行。
    pub fn release_rows(&mut self) {
        self.rows.clear();
        self.chunks = HashSet::new();
    }

    /// 应用新的结果集顺序：按 `ResultId` 复用已有的行，把它们挪到新下标。
    ///
    /// 返回复用的行数：为 0 说明这次是全新的结果集（换了查询），调用方可以把选中复位。
    pub fn apply_results(&mut self, ids: Arc<Vec<ResultId>>) -> usize {
        let mut previous: HashMap<ResultId, RowData> = std::mem::take(&mut self.rows)
            .into_values()
            .map(|row| (row.result, row))
            .collect();
        let mut rows = BTreeMap::new();
        for (index, id) in ids.iter().enumerate() {
            if let Some(mut row) = previous.remove(id) {
                row.index = index as u32;
                rows.insert(index as u32, row);
            }
        }
        let reused = rows.len();
        self.rows = rows;
        self.ids = ids;
        self.refresh_chunks();
        reused
    }

    /// 索引版本变化后重新取该根的行。节点 ID 相同也可能有新的元数据。
    pub fn invalidate_roots(&mut self, roots: &[RootId]) {
        self.rows
            .retain(|_, row| !roots.contains(&row.result.root()));
        self.refresh_chunks();
    }

    /// 已加载行按批聚合，只遍历有缓存的行。
    fn refresh_chunks(&mut self) {
        self.chunks.clear();
        let total = self.ids.len() as u32;
        let mut counts = HashMap::<u32, u32>::new();
        for &index in self.rows.keys() {
            *counts.entry(index / CHUNK).or_default() += 1;
        }
        for (chunk, count) in counts {
            if count == total.saturating_sub(chunk * CHUNK).min(CHUNK) {
                self.chunks.insert(chunk);
            }
        }
    }

    pub fn insert(&mut self, row: RowData) {
        // 结果集已经换过时丢弃迟到的旧响应，否则旧行会落到新结果的下标上。
        if !self.ids.is_empty() && self.ids.get(row.index as usize) != Some(&row.result) {
            return;
        }
        if self.rows.len() >= MAX_ROWS && !self.rows.contains_key(&row.index) {
            let first = *self.rows.first_key_value().unwrap().0;
            let last = *self.rows.last_key_value().unwrap().0;
            let farthest = if first.abs_diff(row.index) >= last.abs_diff(row.index) {
                first
            } else {
                last
            };
            let chunk = farthest / CHUNK;
            for offset in 0..CHUNK {
                self.rows.remove(&(chunk * CHUNK + offset));
            }
            self.chunks.remove(&chunk);
        }
        self.chunks.insert(row.index / CHUNK);
        self.rows.insert(row.index, row);
    }

    pub fn get(&self, index: u32) -> Option<&RowData> {
        self.rows.get(&index)
    }

    /// 可见范围内还没请求过的批，返回 `(start, count)` 请求列表。
    pub fn take_requests(&mut self, visible: (usize, usize), total: usize) -> Vec<(u32, u32)> {
        let (start, end) = visible;
        let end = end.min(total);
        if end <= start {
            return Vec::new();
        }
        let first = (start as u32) / CHUNK;
        let last = ((end as u32).saturating_sub(1)) / CHUNK;
        let mut requests = Vec::new();
        for chunk in first..=last {
            if self.chunks.insert(chunk) {
                requests.push((chunk * CHUNK, CHUNK));
            }
        }
        requests
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::protocol::RowData;
    use devtools_search_core::{NodeId, NodeKind, ResultId, RootId};

    fn row(index: u32) -> RowData {
        RowData {
            index,
            missing: false,
            result: ResultId::new(RootId(0), NodeId(index), 1),
            name: format!("f{index}"),
            path: format!("/f{index}"),
            parent: "/".into(),
            size: Some(1),
            mtime: None,
            btime: None,
            kind: NodeKind::File,
        }
    }

    fn id(node: u32) -> ResultId {
        ResultId::new(RootId(0), NodeId(node), 1)
    }

    #[test]
    fn requests_each_chunk_once() {
        let mut cache = RowCache::default();
        let first = cache.take_requests((0, 10), 1000);
        assert_eq!(first, vec![(0, CHUNK)]);
        // 同一范围不重复请求。
        assert!(cache.take_requests((0, 10), 1000).is_empty());
        // 跨批范围请求两个批。
        let next = cache.take_requests((0, 200), 1000);
        assert_eq!(next, vec![(CHUNK, CHUNK)]);
    }

    #[test]
    fn clear_resets_requests_and_rows() {
        let mut cache = RowCache::default();
        cache.insert(row(5));
        assert!(cache.get(5).is_some());
        cache.clear();
        assert!(cache.get(5).is_none());
        assert_eq!(cache.take_requests((0, 1), 10).len(), 1);
    }

    #[test]
    fn releasing_rows_allows_reloading_the_same_results() {
        let mut cache = RowCache::default();
        cache.apply_results(Arc::new(vec![id(0)]));
        cache.insert(row(0));
        assert!(cache.take_requests((0, 1), 1).is_empty());
        cache.release_rows();
        assert!(cache.get(0).is_none());
        assert_eq!(cache.take_requests((0, 1), 1), [(0, CHUNK)]);
        let mut stale = row(0);
        stale.result = id(1);
        cache.insert(stale);
        assert!(cache.get(0).is_none());
        cache.insert(row(0));
        assert!(cache.get(0).is_some());
    }

    #[test]
    fn new_contents_survive_result_reorder() {
        let mut cache = RowCache::default();
        let ids = Arc::new((0..3).map(id).collect::<Vec<_>>());
        cache.apply_results(ids.clone());
        for index in 0..3 {
            let mut row = row(index);
            row.result = ids[index as usize];
            cache.insert(row);
        }
        // 前面插进来两行：老行搬到 2..5，内容跟着走。
        let shifted: Vec<ResultId> = [id(100), id(101), id(0), id(1), id(2)].to_vec();
        let reused = cache.apply_results(Arc::new(shifted));
        assert_eq!(reused, 3);
        assert_eq!(cache.get(0).map(|r| r.name.as_str()), None);
        assert_eq!(cache.get(2).map(|r| r.name.as_str()), Some("f0"));
        assert_eq!(cache.get(2).map(|r| r.index), Some(2));
        assert_eq!(cache.get(4).map(|r| r.name.as_str()), Some("f2"));
        // 只搬进来一部分的批要重新请求（第 0 批缺 index 0/1）。
        let requests = cache.take_requests((0, 5), 5);
        assert_eq!(requests, vec![(0, CHUNK)]);
    }

    #[test]
    fn applying_unrelated_results_keeps_nothing() {
        let mut cache = RowCache::default();
        cache.apply_results(Arc::new(vec![id(0), id(1)]));
        cache.insert(row(0));
        let reused = cache.apply_results(Arc::new(vec![id(7), id(8)]));
        assert_eq!(reused, 0);
        assert!(cache.get(0).is_none());
        // 迟到的旧响应被丢弃。
        cache.insert(row(1));
        assert!(cache.get(1).is_none());
    }
}
