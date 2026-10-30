//! 可见行缓存（§7.10 LoadRows：128 行一批）。
//!
//! 结果集与代数变化时整体清空；缓存有上限，滚动长时间浏览不会无限增长。

use std::collections::{BTreeMap, HashSet};

use crate::application::protocol::RowData;

/// 一批的行数。
pub const CHUNK: u32 = 128;
/// 缓存行上限（超过后保留最近一半）。
const MAX_ROWS: usize = 4096;

#[derive(Default)]
pub struct RowCache {
    rows: BTreeMap<u32, RowData>,
    /// 已请求的批号。
    chunks: HashSet<u32>,
}

impl RowCache {
    pub fn clear(&mut self) {
        self.rows.clear();
        self.chunks.clear();
    }

    pub fn insert(&mut self, row: RowData) {
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
    use devtools_search_core::{NodeKind, ResultId, RootId, NodeId};

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
}
