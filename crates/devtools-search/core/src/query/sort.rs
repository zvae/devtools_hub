//! 排序与 top-k（§7.9）。
//!
//! 默认是路径序（rank），不能用 NodeId 顺序代替；文件与目录混排。
//! 显式按大小/时间/名字排序时：对完整匹配集排序再取 top-k（0 = 不限）；
//! 未知元数据统一置后（与升降序无关）。

use std::cmp::Ordering;

use crate::index::RootIndex;
use crate::model::{NodeId, NodeMeta, SortColumn, SortDir};

/// 排序并截取结果；返回 (截取后的节点列表, 完整匹配数)。
pub fn sort_matches(
    index: &RootIndex,
    mut ids: Vec<NodeId>,
    column: SortColumn,
    dir: SortDir,
    limit: usize,
) -> (Vec<NodeId>, usize) {
    let total = ids.len();
    match (column, dir) {
        (SortColumn::Path, _) | (_, SortDir::None) => {
            ids.sort_by_key(|&n| index.rank_of(n));
        }
        (col, dir) => {
            ids.sort_by(|a, b| compare(index, *a, *b, col, dir));
        }
    }
    if limit > 0 && ids.len() > limit {
        ids.truncate(limit);
    }
    (ids, total)
}

/// 两个节点在显式排序下的比较：未知元数据统一置后（与方向无关，§7.9），
/// 相同主键按 rank 保证确定性。
fn compare(index: &RootIndex, a: NodeId, b: NodeId, column: SortColumn, dir: SortDir) -> Ordering {
    let (ma, mb) = (index.meta(a), index.meta(b));
    let (missing_a, missing_b) = missing(column, ma, mb);
    let rank_tie = || index.rank_of(a).cmp(&index.rank_of(b));
    // Unknown 统一置后（与方向无关）：缺失比较不参与方向反转。
    let missing_ord = missing_a.cmp(&missing_b);
    if missing_ord != Ordering::Equal {
        return missing_ord.then_with(rank_tie);
    }
    let ord = match column {
        SortColumn::Name => index.sort_name(a).cmp(index.sort_name(b)),
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
    ord.then_with(rank_tie)
}

/// 元数据是否缺失（Unknown 置后标志）；名字列无缺失。
fn missing(column: SortColumn, ma: NodeMeta, mb: NodeMeta) -> (bool, bool) {
    match column {
        SortColumn::Size => (ma.size.is_none(), mb.size.is_none()),
        SortColumn::Modified => (ma.mtime.is_none(), mb.mtime.is_none()),
        SortColumn::Created => (ma.btime.is_none(), mb.btime.is_none()),
        SortColumn::Name | SortColumn::Path => (false, false),
    }
}
