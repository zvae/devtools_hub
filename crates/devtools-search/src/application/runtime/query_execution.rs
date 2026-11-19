//! 查询解析、跨根 top-k 与稳定排序键；与扫描和监听生命周期分离。

use super::{QueryOutcome, QuerySpec, RootSlot};
use devtools_search_core::query::{self, Ast, EvalOptions, Term};
use devtools_search_core::volumes::RootState;
use devtools_search_core::{FileKind, NodeId, ResultId, RootId, RootIndex, SortColumn, SortDir};
use std::cmp::Ordering as CmpOrdering;
use std::collections::BinaryHeap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(super) fn run_query(
    slots: &[Arc<RootSlot>],
    spec: &QuerySpec,
    cancel: &AtomicBool,
) -> QueryOutcome {
    if cancel.load(Ordering::Relaxed) {
        return QueryOutcome::cancelled();
    }
    // 空查询：只有"列出全部"、指定了目录范围或选了具体类型时才继续
    // （范围与类型本身就是过滤条件）。
    if spec.text.trim().is_empty()
        && !spec.show_all_on_empty
        && spec.scope.is_none()
        && spec.kind == FileKind::All
    {
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
    // 附加条件：类型筛选与目录范围（都是与查询本身 AND 的过滤项）。
    let mut terms = vec![ast];
    if let Some(kind_ast) = spec.kind.to_ast() {
        terms.push(kind_ast);
    }
    if let Some(scope) = &spec.scope {
        // 入口带选中的文件夹时作为 infolder 过滤（§4.5）。
        terms.push(Ast::Term(Term::InFolder(
            scope.to_string_lossy().into_owned(),
        )));
    }
    let ast = if terms.len() == 1 {
        terms.pop().unwrap()
    } else {
        Ast::And(terms)
    };
    let limit = if spec.max_results == 0 {
        usize::MAX
    } else {
        spec.max_results as usize
    };
    let path_order = spec.sort_column == SortColumn::Path || spec.sort_dir == SortDir::None;
    let mut roots: Vec<_> = slots
        .iter()
        .filter(|slot| !matches!(slot.state(), RootState::Offline | RootState::Dormant))
        .collect();
    if path_order {
        roots.sort_by(|a, b| a.mount.cmp(&b.mount));
    }
    let mut total = 0;
    let mut notice = None;
    let mut best = BinaryHeap::<Candidate>::new();
    for (position, slot) in roots.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return QueryOutcome::cancelled();
        }
        // 每根只在匹配及提取稳定排序键时持锁。候选项带 epoch，旧节点不会冒充新节点。
        let index = slot.index.read().unwrap();
        let opts = EvalOptions {
            case_sensitive: spec.case_sensitive,
            home: spec.home.clone(),
            root_abs: slot.mount.clone(),
        };
        let root_node = index.root_node();
        let (root_notice, cancelled) = query::visit_matches(&index, &ast, &opts, cancel, |node| {
            if Some(node) == root_node {
                return;
            }
            total += 1;
            if best.len() < limit {
                best.push(Candidate::new(
                    &index, node, slot.id, position, spec, path_order,
                ));
            } else if best.peek().is_some_and(|worst| {
                worst.compare_node(&index, node, position) == CmpOrdering::Greater
            }) {
                // 名字排序只为进入前 K 的候选复制字符串，落选项不分配排序键。
                *best.peek_mut().unwrap() =
                    Candidate::new(&index, node, slot.id, position, spec, path_order);
            }
        });
        if cancelled || cancel.load(Ordering::Relaxed) {
            return QueryOutcome::cancelled();
        }
        if let Some(query::Notice::NotIndexed { value }) = root_notice {
            notice.get_or_insert(format!("{value} 不在索引范围内，可把它添加为索引根"));
        }
    }
    // 堆序输出无需再访问索引，且长排序可以分段取消。
    let mut ids = Vec::with_capacity(best.len());
    while let Some(candidate) = best.pop() {
        if ids.len() % 1024 == 0 && cancel.load(Ordering::Relaxed) {
            return QueryOutcome::cancelled();
        }
        ids.push(candidate.id);
    }
    ids.reverse();
    QueryOutcome {
        ids,
        total,
        error: None,
        notice,
        cancelled: false,
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum PrimaryKey {
    Path,
    Name(String),
    Size(u64),
    Time(i64),
}

struct Candidate {
    id: ResultId,
    key: PrimaryKey,
    missing: bool,
    rank: u32,
    root_position: usize,
    descending: bool,
    path_order: bool,
    column: SortColumn,
}

impl Candidate {
    fn compare_node(&self, index: &RootIndex, node: NodeId, root_position: usize) -> CmpOrdering {
        let rank = index.rank_of(node);
        if self.path_order {
            return self
                .root_position
                .cmp(&root_position)
                .then_with(|| self.rank.cmp(&rank));
        }
        let meta = index.meta(node);
        let (order, missing) = match &self.key {
            PrimaryKey::Name(name) => (name.as_str().cmp(index.sort_name(node)), false),
            PrimaryKey::Size(size) => (size.cmp(&meta.size.unwrap_or(0)), meta.size.is_none()),
            PrimaryKey::Time(time) => {
                // Modified 和 Created 共用数值类型；调用方保存具体列。
                let value = if self.column == SortColumn::Created {
                    meta.btime
                } else {
                    meta.mtime
                };
                (time.cmp(&value.unwrap_or(0)), value.is_none())
            }
            PrimaryKey::Path => (CmpOrdering::Equal, false),
        };
        self.missing
            .cmp(&missing)
            .then_with(|| {
                if self.descending {
                    order.reverse()
                } else {
                    order
                }
            })
            .then_with(|| self.rank.cmp(&rank))
            .then_with(|| self.root_position.cmp(&root_position))
    }

    fn new(
        index: &RootIndex,
        node: NodeId,
        root: RootId,
        root_position: usize,
        spec: &QuerySpec,
        path_order: bool,
    ) -> Self {
        let meta = index.meta(node);
        let (key, missing) = if path_order {
            (PrimaryKey::Path, false)
        } else {
            match spec.sort_column {
                SortColumn::Name => (PrimaryKey::Name(index.sort_name(node).to_owned()), false),
                SortColumn::Size => (
                    PrimaryKey::Size(meta.size.unwrap_or(0)),
                    meta.size.is_none(),
                ),
                SortColumn::Modified => (
                    PrimaryKey::Time(meta.mtime.unwrap_or(0)),
                    meta.mtime.is_none(),
                ),
                SortColumn::Created => (
                    PrimaryKey::Time(meta.btime.unwrap_or(0)),
                    meta.btime.is_none(),
                ),
                SortColumn::Path => (PrimaryKey::Path, false),
            }
        };
        Self {
            id: ResultId::new(root, node, index.epoch()),
            key,
            missing,
            rank: index.rank_of(node),
            root_position,
            descending: spec.sort_dir == SortDir::Desc,
            path_order,
            column: spec.sort_column,
        }
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        if self.path_order {
            return self
                .root_position
                .cmp(&other.root_position)
                .then_with(|| self.rank.cmp(&other.rank));
        }
        self.missing
            .cmp(&other.missing)
            .then_with(|| {
                let order = self.key.cmp(&other.key);
                if self.descending {
                    order.reverse()
                } else {
                    order
                }
            })
            .then_with(|| self.rank.cmp(&other.rank))
            .then_with(|| self.root_position.cmp(&other.root_position))
    }
}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == CmpOrdering::Equal
    }
}
impl Eq for Candidate {}
