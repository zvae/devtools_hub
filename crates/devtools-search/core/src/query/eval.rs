//! 查询求值（§7.8）。
//!
//! 名字词先匹配驻留名字（arena 文本），再展开节点；大小/时间只读取内存元数据，
//! 查询阶段禁止按候选逐个 stat（§7.3）；路径词先匹配最后一段名字，再沿 parent 链逐段
//! 验证；`infolder:` 解析成目录节点、以 DFS `[tin, tout)` 判后代；`parent:` 直接比较
//! 父节点。所有查询可取消。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::ast::{Ast, DirRef, SizeFilter, Term};
use crate::index::RootIndex;
use crate::model::{NodeKind, NodeId};

/// 求值选项。
pub struct EvalOptions {
    /// Aa：区分大小写（true）或 NFC+lower（false），§7.1。
    pub case_sensitive: bool,
    /// 解析 `~` 用的主目录；`None` 时 `~` 前缀无法展开（按不在索引范围处理）。
    pub home: Option<PathBuf>,
    /// 当前索引根对应的绝对路径。
    pub root_abs: PathBuf,
}

/// 求值结果附带的通知（不算错误，§7.8：返回空结果并提示）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// `infolder:` / `parent:` 指向未配置索引的目录。
    NotIndexed { value: String },
}

/// 求值结果。
pub struct EvalOutcome {
    pub matches: Vec<NodeId>,
    pub notice: Option<Notice>,
    /// 取消标志（结果不完整，调用方应丢弃）。
    pub cancelled: bool,
}

/// 一次查询的编译产物：模式已按大小写设置归一化、目录引用已解析。
pub enum Compiled {
    And(Vec<Compiled>),
    Or(Vec<Compiled>),
    Not(Box<Compiled>),
    All,
    Term(CompiledTerm),
}

pub enum CompiledTerm {
    /// `case` 与模式归一化时一致：名字文本按同一规则取（§7.1）。
    Name { pattern: String, wildcards: bool, case: bool },
    Path { segments: Vec<(String, bool)>, case: bool },
    Ext(Vec<String>),
    Type(Vec<String>),
    File,
    Folder,
    Size(SizeFilter),
    DateM(super::ast::DateFilter),
    DateC(super::ast::DateFilter),
    InFolder(DirRef),
    Parent(DirRef),
    Regex(regex::Regex),
}

/// 编译：解析目录引用、按大小写设置归一化模式。
pub fn compile(index: &RootIndex, ast: &Ast, opts: &EvalOptions) -> (Compiled, Option<Notice>) {
    let mut notice = None;
    let compiled = compile_node(index, ast, opts, &mut notice);
    (compiled, notice)
}

fn compile_node(
    index: &RootIndex,
    ast: &Ast,
    opts: &EvalOptions,
    notice: &mut Option<Notice>,
) -> Compiled {
    match ast {
        Ast::And(terms) => Compiled::And(
            terms
                .iter()
                .map(|t| compile_node(index, t, opts, notice))
                .collect(),
        ),
        Ast::Or(terms) => Compiled::Or(
            terms
                .iter()
                .map(|t| compile_node(index, t, opts, notice))
                .collect(),
        ),
        Ast::Not(inner) => Compiled::Not(Box::new(compile_node(index, inner, opts, notice))),
        Ast::Term(term) => {
            if matches!(
                term,
                Term::InFolder(_) | Term::Parent(_)
            ) {
                // 目录引用解析失败时记录一次通知。
                if let Some(value) = dir_value(term) {
                    if resolve_dir(index, &value, opts) == DirRef::NotIndexed
                        && notice.is_none()
                    {
                        *notice = Some(Notice::NotIndexed { value });
                    }
                }
            }
            Compiled::Term(compile_term(index, term, opts))
        }
    }
}

fn dir_value(term: &Term) -> Option<String> {
    match term {
        Term::InFolder(v) | Term::Parent(v) => Some(v.clone()),
        _ => None,
    }
}

fn compile_term(index: &RootIndex, term: &Term, opts: &EvalOptions) -> CompiledTerm {
    let case = opts.case_sensitive;
    let norm = |s: &str| crate::text::match_text_of(s, case);
    match term {
        Term::Name { pattern, wildcards } => CompiledTerm::Name {
            pattern: norm(pattern),
            wildcards: *wildcards,
            case,
        },
        Term::Path(segments) => CompiledTerm::Path {
            segments: segments
                .iter()
                .map(|s| (norm(&s.pattern), s.wildcards))
                .collect(),
            case,
        },
        Term::Ext(exts) => CompiledTerm::Ext(
            exts.iter()
                .map(|e| crate::text::fold(e.trim_start_matches('.')))
                .collect(),
        ),
        Term::Type(group) => {
            let exts = super::types::extensions_for(group).unwrap_or_default();
            CompiledTerm::Type(exts.iter().map(|e| e.to_string()).collect())
        }
        Term::File => CompiledTerm::File,
        Term::Folder => CompiledTerm::Folder,
        Term::Size(f) => CompiledTerm::Size(*f),
        Term::DateModified(f) => CompiledTerm::DateM(*f),
        Term::DateCreated(f) => CompiledTerm::DateC(*f),
        Term::InFolder(v) => CompiledTerm::InFolder(resolve_dir(index, v, opts)),
        Term::Parent(v) => CompiledTerm::Parent(resolve_dir(index, v, opts)),
        Term::Regex(pattern) => {
            // 正则单独用 Unicode 忽略大小写语义（与普通文本词的 Aa 规则分开，§7.1）。
            let compiled = regex::RegexBuilder::new(pattern)
                .case_insensitive(!case)
                .build()
                .unwrap_or_else(|_| regex::Regex::new("$^").unwrap());
            CompiledTerm::Regex(compiled)
        }
    }
}

/// 求值入口（compile + 遍历）。空 AST 匹配全部（是否执行由 `show_all_on_empty` 决定）。
pub fn eval(index: &RootIndex, ast: &Ast, opts: &EvalOptions, cancel: &AtomicBool) -> EvalOutcome {
    let (compiled, notice) = compile(index, ast, opts);
    eval_compiled(index, &compiled, cancel, notice)
}

pub fn eval_compiled(
    index: &RootIndex,
    compiled: &Compiled,
    cancel: &AtomicBool,
    notice: Option<Notice>,
) -> EvalOutcome {
    let mut matches = Vec::new();
    let mut cancelled = false;
    for node in 0..index.len() {
        if (node & 0x3FF) == 0 && cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let node = NodeId(node);
        if !index.status(node).visible() {
            continue;
        }
        if matches_compiled(index, compiled, node) {
            matches.push(node);
        }
    }
    EvalOutcome {
        matches,
        notice,
        cancelled,
    }
}

fn matches_compiled(index: &RootIndex, compiled: &Compiled, node: NodeId) -> bool {
    match compiled {
        Compiled::All => true,
        Compiled::And(terms) => terms
            .iter()
            .all(|t| matches_compiled(index, t, node)),
        Compiled::Or(terms) => terms
            .iter()
            .any(|t| matches_compiled(index, t, node)),
        Compiled::Not(inner) => !matches_compiled(index, inner, node),
        Compiled::Term(term) => matches_term(index, term, node),
    }
}

fn matches_term(index: &RootIndex, term: &CompiledTerm, node: NodeId) -> bool {
    match term {
        CompiledTerm::Name { pattern, wildcards, case } => {
            let Some(text) = index.match_text(node, *case) else {
                return false;
            };
            if *wildcards {
                wildcard_match(pattern, text)
            } else {
                text.contains(pattern.as_str())
            }
        }
        CompiledTerm::Path { segments, case } => {
            // 先匹配最后一段名字，再沿 parent 链逐段验证（连续向上，§7.8）。
            if segments.is_empty() {
                return false;
            }
            if !seg_matches(index, node, &segments[segments.len() - 1], *case) {
                return false;
            }
            let mut cur = node;
            for seg in segments.iter().rev().skip(1) {
                let Some(parent) = index.parent(cur) else {
                    return false;
                };
                if !seg_matches(index, parent, seg, *case) {
                    return false;
                }
                cur = parent;
            }
            true
        }
        CompiledTerm::Ext(exts) => {
            if index.kind(node) != NodeKind::File {
                return false;
            }
            let ext = file_ext(index, node);
            ext.is_some_and(|e| exts.contains(&e))
        }
        CompiledTerm::Type(exts) => {
            if index.kind(node) != NodeKind::File {
                return false;
            }
            let ext = file_ext(index, node);
            ext.is_some_and(|e| exts.contains(&e))
        }
        CompiledTerm::File => index.kind(node) == NodeKind::File,
        CompiledTerm::Folder => index.kind(node) == NodeKind::Dir,
        CompiledTerm::Size(filter) => {
            let Some(size) = index.meta(node).size else {
                return false; // Unknown 不命中需要该字段的过滤器（§7.8）。
            };
            match *filter {
                SizeFilter::Gt(v) => size > v,
                SizeFilter::Lt(v) => size < v,
                SizeFilter::Range(a, b) => size >= a && size <= b,
                SizeFilter::Exact(v) => size == v,
            }
        }
        CompiledTerm::DateM(f) => f.contains(index.meta(node).mtime),
        CompiledTerm::DateC(f) => f.contains(index.meta(node).btime),
        CompiledTerm::InFolder(dir) => match dir {
            DirRef::Node(dir) => index.is_descendant(*dir, node),
            DirRef::NotIndexed => false,
        },
        CompiledTerm::Parent(dir) => match dir {
            DirRef::Node(dir) => index.parent(node) == Some(*dir),
            DirRef::NotIndexed => false,
        },
        CompiledTerm::Regex(re) => index
            .match_text(node, true)
            .map(|text| re.is_match(text))
            .unwrap_or(false),
    }
}

/// 一段路径模式对节点名字的匹配（子串或整名通配符）。
fn seg_matches(index: &RootIndex, node: NodeId, seg: &(String, bool), case: bool) -> bool {
    let (pattern, wildcards) = seg;
    let text = index.match_text(node, case).unwrap_or("");
    if *wildcards {
        wildcard_match(pattern, text)
    } else {
        text.contains(pattern.as_str())
    }
}

/// 扩展名（小写、不含点）。
fn file_ext(index: &RootIndex, node: NodeId) -> Option<String> {
    let name = index.match_text(node, false)?;
    let dot = name.rfind('.')?;
    let ext = &name[dot + 1..];
    if ext.is_empty() || name.starts_with('.') && name.rfind('.') == Some(0) {
        // 隐藏文件没有扩展名（".git" 的 ext 是 None）。
        return None;
    }
    Some(ext.to_string())
}

/// 解析 `infolder:` / `parent:` 的目录值到索引节点。
fn resolve_dir(index: &RootIndex, value: &str, opts: &EvalOptions) -> DirRef {
    let path = expand_path(value, opts);
    let Some(path) = path else {
        return DirRef::NotIndexed;
    };
    // 计算相对根的段。
    let segments: Vec<String> = match path.strip_prefix(&opts.root_abs) {
        Ok(rel) => rel
            .components()
            .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
            .collect(),
        Err(_) => return DirRef::NotIndexed,
    };
    let mut cur = match index.root_node() {
        Some(root) => root,
        None => return DirRef::NotIndexed,
    };
    for seg in &segments {
        if seg.is_empty() {
            continue;
        }
        let target = crate::text::fold(seg);
        let found = index
            .children(cur)
            .iter()
            .find(|&child| {
                index.kind(*child) == NodeKind::Dir
                    && index
                        .raw_name_str(*child)
                        .is_some_and(|n| crate::text::fold(n) == target)
            })
            .copied();
        match found {
            Some(next) => cur = next,
            None => return DirRef::NotIndexed,
        }
    }
    DirRef::Node(cur)
}

/// 展开 `~` 并转成绝对路径；相对路径相对索引根。
fn expand_path(value: &str, opts: &EvalOptions) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(rest) = value.strip_prefix('~') {
        let home = opts.home.as_ref()?;
        let rest = rest.trim_start_matches('/');
        return Some(if rest.is_empty() {
            home.clone()
        } else {
            home.join(rest)
        });
    }
    let path = Path::new(value);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        Some(opts.root_abs.join(path))
    }
}

/// 字符区间的通配符匹配（`*` 任意长度、`?` 单字符；按字符而非字节，避免切半多字节字符）。
pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut si, mut pi) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while si < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = si;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}
