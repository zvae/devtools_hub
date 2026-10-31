//! 可见行缓存（§7.10 LoadRows：128 行一批）。
//!
//! 结果集变化时按 `ResultId` 增量重排：仍在新结果里的行只改下标、内容保留，
//! 只有真正新增/消失的行才留下空洞。索引更新期间重跑查询不会把整张表清空再重填，
//! 界面也就不会整屏闪一下。缓存有上限，滚动长时间浏览不会无限增长。

use std::collections::{BTreeMap, HashMap, HashSet};

use devtools_search_core::ResultId;

use crate::application::protocol::RowData;

/// 一批的行数。
pub const CHUNK: u32 = 128;
/// 缓存行上限（超过后保留最近一半）。
const MAX_ROWS: usize = 4096;

#[derive(Default)]
pub struct RowCache {
    rows: BTreeMap<u32, RowData>,
    /// 已请求（或已完整加载）的批号。
    chunks: HashSet<u32>,
    /// 当前结果集的顺序：增量重排的依据，也用来丢弃迟到的旧响应。
    ids: Vec<ResultId>,
}

impl RowCache {
    pub fn clear(&mut self) {
        self.rows.clear();
        self.chunks.clear();
        self.ids.clear();
    }

    /// 应用新的结果集顺序：按 `ResultId` 复用已有的行，把它们挪到新下标。
    ///
    /// 返回复用的行数：为 0 说明这次是全新的结果集（换了查询），调用方可以把选中复位。
    pub fn apply_results(&mut self, ids: &[ResultId]) -> usize {
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
        self.ids = ids.to_vec();
        self.refresh_chunks();
        reused
    }

    /// 重新计算哪些批已经完全加载：被重排撕开的批（只搬进来一部分行）要重新请求。
    fn refresh_chunks(&mut self) {
        self.chunks.clear();
        let total = self.ids.len() as u32;
        let mut chunk = 0;
        while chunk * CHUNK < total {
            let start = chunk * CHUNK;
            let end = (start + CHUNK).min(total);
            if (start..end).all(|index| self.rows.contains_key(&index)) {
                self.chunks.insert(chunk);
            }
            chunk += 1;
        }
    }

    pub fn insert(&mut self, row: RowData) {
        // 结果集已经换过时丢弃迟到的旧响应，否则旧行会落到新结果的下标上。
        if !self.ids.is_empty() && self.ids.get(row.index as usize) != Some(&row.result) {
            return;
        }
        if self.rows.len() > MAX_ROWS {
            // 丢弃前半部分（用户很少回滚这么远）。
            let cut = (self.rows.len() / 2) as u32;
            self.rows = self.rows.split_off(&cut);
            self.chunks.retain(|&c| c >= cut / CHUNK);
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
            result: ResultId::new(RootId(0), NodeId(index)),
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
        ResultId::new(RootId(0), NodeId(node))
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
    fn new_contents_survive_result_reorder() {
        let mut cache = RowCache::default();
        let ids: Vec<ResultId> = (0..3).map(id).collect();
        cache.apply_results(&ids);
        for index in 0..3 {
            let mut row = row(index);
            row.result = ids[index as usize];
            cache.insert(row);
        }
        // 前面插进来两行：老行搬到 2..5，内容跟着走。
        let shifted: Vec<ResultId> = [id(100), id(101), id(0), id(1), id(2)].to_vec();
        let reused = cache.apply_results(&shifted);
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
        cache.apply_results(&[id(0), id(1)]);
        cache.insert(row(0));
        let reused = cache.apply_results(&[id(7), id(8)]);
        assert_eq!(reused, 0);
        assert!(cache.get(0).is_none());
        // 迟到的旧响应被丢弃。
        cache.insert(row(1));
        assert!(cache.get(1).is_none());
    }
}
