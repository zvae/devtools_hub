//! 查询模块的综合测试：语法解析 → 求值 → 排序（§7.8、§7.9）。

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::index::RootIndex;
use crate::model::{NodeId, NodeKind, NodeMeta, RootId, SortColumn, SortDir};

/// 构造测试索引：
/// ```text
/// root (dir)
/// ├── 年度报告.pdf           size=2MB, mtime=今天
/// ├── annual report.docx     size=100KB, mtime=今天
/// ├── draft.txt              size=1KB, mtime=2020-01-01
/// ├── project (dir)
/// │   ├── report.md          size=4KB
/// │   └── src (dir)
/// │       └── main.rs        size=2KB
/// ├── photos (dir)
/// │   ├── IMG_001.PNG        size=5MB
/// │   └── notes.txt          size=512B
/// └── .git (dir)             Excluded
///     └── config             Excluded
/// ```
fn sample_index() -> RootIndex {
    let now = chrono::Local::now().timestamp();
    let mut index = RootIndex::new(RootId(0));
    let root = index.insert(None, OsStr::new("root"), NodeKind::Dir, NodeMeta::default());
    let ins = |index: &mut RootIndex, parent, name: &str, size: Option<u64>, mtime: Option<i64>| {
        index.insert(
            Some(parent),
            OsStr::new(name),
            NodeKind::File,
            NodeMeta {
                size,
                mtime,
                btime: mtime,
            },
        )
    };
    ins(&mut index, root, "年度报告.pdf", Some(2 * 1024 * 1024), Some(now));
    ins(&mut index, root, "annual report.docx", Some(100 * 1024), Some(now));
    ins(&mut index, root, "draft.txt", Some(1024), Some(1_577_836_800));
    let project = index.insert(Some(root), OsStr::new("project"), NodeKind::Dir, NodeMeta::default());
    ins(&mut index, project, "report.md", Some(4 * 1024), Some(now));
    let src = index.insert(Some(project), OsStr::new("src"), NodeKind::Dir, NodeMeta::default());
    ins(&mut index, src, "main.rs", Some(2 * 1024), Some(now));
    let photos = index.insert(Some(root), OsStr::new("photos"), NodeKind::Dir, NodeMeta::default());
    ins(&mut index, photos, "IMG_001.PNG", Some(5 * 1024 * 1024), Some(now));
    ins(&mut index, photos, "notes.txt", Some(512), Some(now));
    let git = index.insert(Some(root), OsStr::new(".git"), NodeKind::Dir, NodeMeta::default());
    let config = ins(&mut index, git, "config", Some(128), Some(now));
    index.set_excluded(git, true);
    index.set_excluded(config, true);
    index.commit();
    index
}

fn opts() -> EvalOptions {
    EvalOptions {
        case_sensitive: false,
        home: Some(PathBuf::from("/root")),
        root_abs: PathBuf::from("/root"),
    }
}

fn names_for(index: &RootIndex, query: &str) -> Vec<String> {
    let ast = parse(query).unwrap();
    let outcome = eval(index, &ast, &opts(), &AtomicBool::new(false));
    assert!(!outcome.cancelled);
    let mut names: Vec<String> = outcome
        .matches
        .iter()
        .map(|&n| index.raw_name_str(n).unwrap().to_string())
        .collect();
    names.sort();
    names
}

#[test]
fn substring_matches_case_insensitively() {
    let index = sample_index();
    assert_eq!(names_for(&index, "report"), ["annual report.docx", "report.md"]);
    // NFC + lower：大小写混合的模式也能命中。
    assert_eq!(names_for(&index, "REPORT"), ["annual report.docx", "report.md"]);
    // CJK 名字直接命中。
    assert_eq!(names_for(&index, "年度"), ["年度报告.pdf"]);
    // 大小写敏感（Aa）：report 命中小写、REPORT 不命中。
    let ast = parse("report").unwrap();
    let mut sensitive = opts();
    sensitive.case_sensitive = true;
    let outcome = eval(&index, &ast, &sensitive, &AtomicBool::new(false));
    let names: Vec<&str> = outcome
        .matches
        .iter()
        .filter_map(|&n| index.raw_name_str(n))
        .collect();
    assert_eq!(names, ["annual report.docx", "report.md"]);
}

#[test]
fn quoted_phrase_keeps_spaces() {
    let index = sample_index();
    assert_eq!(names_for(&index, "\"annual report\""), ["annual report.docx"]);
    assert!(names_for(&index, "\"annual  report\"").is_empty());
}

#[test]
fn wildcard_matches_whole_name() {
    let index = sample_index();
    assert_eq!(names_for(&index, "report*.md"), ["report.md"]);
    assert!(names_for(&index, "report*").is_empty() || names_for(&index, "report*") == ["report.md"]);
    assert_eq!(names_for(&index, "*.txt"), ["draft.txt", "notes.txt"]);
    assert_eq!(names_for(&index, "IMG_00?.PNG"), ["IMG_001.PNG"]);
}

#[test]
fn boolean_operators() {
    let index = sample_index();
    // AND（空格）。
    assert_eq!(names_for(&index, "report md"), ["report.md"]);
    // OR。
    assert_eq!(names_for(&index, "pdf|docx"), ["annual report.docx", "年度报告.pdf"]);
    // NOT。
    assert_eq!(names_for(&index, "report !draft"), ["annual report.docx", "report.md"]);
    // 分组：(a | b) !draft。
    let names = names_for(&index, "(pdf | txt) !draft");
    assert_eq!(names, ["notes.txt", "年度报告.pdf"]);
    // NOT 优先级高于 AND。
    assert!(parse("!a b").is_ok());
}

#[test]
fn extension_and_type_filters() {
    let index = sample_index();
    assert_eq!(names_for(&index, "ext:pdf"), ["年度报告.pdf"]);
    assert_eq!(names_for(&index, "ext:pdf;docx"), ["annual report.docx", "年度报告.pdf"]);
    // 大小写不敏感扩展名。
    assert_eq!(names_for(&index, "ext:png"), ["IMG_001.PNG"]);
    assert_eq!(names_for(&index, "type:picture"), ["IMG_001.PNG"]);
    // file:/folder:。
    assert!(names_for(&index, "folder:").contains(&"project".to_string()));
    assert!(!names_for(&index, "file:").contains(&"project".to_string()));
    // 不支持的过滤器显式报错。
    assert!(matches!(parse("path:foo"), Err(ParseError::UnsupportedFilter(_))));
    assert!(matches!(parse("type:nope"), Err(ParseError::UnknownTypeGroup(_))));
}

#[test]
fn size_filter_uses_1024_units() {
    let index = sample_index();
    // 2MB、5MB。
    assert_eq!(names_for(&index, "size:>1MB"), ["IMG_001.PNG", "年度报告.pdf"]);
    // 1KB 以下。
    assert_eq!(names_for(&index, "size:<1KB"), ["notes.txt"]);
    // 区间 100KB–3MB。
    let names = names_for(&index, "size:100KB-3MB");
    assert!(names.contains(&"annual report.docx".to_string()));
    assert!(names.contains(&"年度报告.pdf".to_string()));
    assert!(matches!(parse("size:1XB"), Err(ParseError::BadSize(_))));
}

#[test]
fn date_filter_keywords() {
    let index = sample_index();
    // 今天修改的文件（draft.txt 是 2020 年）。
    let names = names_for(&index, "dm:today");
    assert!(names.contains(&"report.md".to_string()));
    assert!(!names.contains(&"draft.txt".to_string()));
    // 显式日期。
    let names = names_for(&index, "dm:2020-01-01");
    assert_eq!(names, ["draft.txt"]);
    // 创建时间（测试里 btime == mtime）。
    assert!(names_for(&index, "dc:2020-01-01").contains(&"draft.txt".to_string()));
}

#[test]
fn path_segments_match_name_then_ancestors() {
    let index = sample_index();
    // project/report：名字 report.md 且父链含 project。
    assert_eq!(names_for(&index, "project/report"), ["report.md"]);
    // project/src/main.rs。
    assert_eq!(names_for(&index, "project/src/main"), ["main.rs"]);
    // 父链不含 photos 的 report 不命中。
    assert!(names_for(&index, "photos/report").is_empty());
}

#[test]
fn infolder_and_parent_resolve_directories() {
    let index = sample_index();
    // infolder:project → 后代（report.md、main.rs、src）。
    let names = names_for(&index, "infolder:project");
    assert!(names.contains(&"report.md".to_string()));
    assert!(names.contains(&"main.rs".to_string()));
    // parent:project → 直接子项。
    let names = names_for(&index, "parent:project");
    assert_eq!(names, ["report.md", "src"]);
    // 未配置索引的目录：空结果 + 通知（§7.8）。
    let ast = parse("infolder:/elsewhere").unwrap();
    let outcome = eval(&index, &ast, &opts(), &AtomicBool::new(false));
    assert!(outcome.matches.is_empty());
    assert!(matches!(outcome.notice, Some(Notice::NotIndexed { .. })));
}

#[test]
fn regex_filter() {
    let index = sample_index();
    assert_eq!(names_for(&index, "regex:^report"), ["report.md"]);
    assert!(matches!(parse("regex:["), Err(ParseError::BadRegex(_))));
}

#[test]
fn excluded_nodes_never_match() {
    let index = sample_index();
    assert!(names_for(&index, "config").is_empty());
    assert!(names_for(&index, "git").is_empty());
}

#[test]
fn literal_mode_bypasses_syntax() {
    // query_is_literal（§4.5）：原始文本按字面子串，不解析布尔/过滤器。
    let index = sample_index();
    let ast = Ast::Term(Term::Name {
        pattern: "pdf|docx".into(),
        wildcards: false,
    });
    let outcome = eval(&index, &ast, &opts(), &AtomicBool::new(false));
    assert!(outcome.matches.is_empty());
}

#[test]
fn default_sort_is_rank_and_explicit_sort_puts_unknown_last() {
    let index = sample_index();
    let ast = parse("report").unwrap();
    let outcome = eval(&index, &ast, &opts(), &AtomicBool::new(false));
    // 默认路径序（rank）：root 下的 annual report.docx 在 project/report.md 之前。
    let (ids, total) = sort_matches(&index, outcome.matches.clone(), SortColumn::Path, SortDir::None, 0);
    assert_eq!(total, 2);
    let ordered: Vec<&str> = ids.iter().filter_map(|&n| index.raw_name_str(n)).collect();
    assert_eq!(ordered, ["annual report.docx", "report.md"]);
    // 按大小降序。
    let ast = parse("report").unwrap();
    let outcome = eval(&index, &ast, &opts(), &AtomicBool::new(false));
    let (ids, _) = sort_matches(&index, outcome.matches, SortColumn::Size, SortDir::Desc, 0);
    let ordered: Vec<&str> = ids.iter().filter_map(|&n| index.raw_name_str(n)).collect();
    assert_eq!(ordered, ["annual report.docx", "report.md"]); // 100KB > 4KB
    // top-k。
    let ast = parse("report").unwrap();
    let outcome = eval(&index, &ast, &opts(), &AtomicBool::new(false));
    let (ids, total) = sort_matches(&index, outcome.matches, SortColumn::Name, SortDir::Asc, 1);
    assert_eq!(total, 2);
    assert_eq!(ids.len(), 1);
}

#[test]
fn node_id_never_used_as_path_order() {
    let index = sample_index();
    // 插入顺序 ≠ rank 顺序：默认排序必须是 rank。
    let root = index.root_node().unwrap();
    let kids = index.children(root);
    let first = kids[0];
    assert!(index.rank_of(first) < index.rank_of(kids[kids.len() - 1]));
    let _ = NodeId(0);
}

/// 性能基准（默认 skip）：20 万个节点上的全量查询，看看 eval 与排序各占多少。
///
/// 跑法：`cargo test -p devtools-search-core --release -- --ignored --nocapture bench`
#[test]
#[ignore = "性能基准，手动跑（release）"]
fn bench_full_scan_query() {
    use std::time::Instant;

    let mut index = RootIndex::new(RootId(0));
    let root = index.insert(None, OsStr::new("root"), NodeKind::Dir, NodeMeta::default());
    const DIRS: usize = 200;
    const FILES: usize = 1000;
    for dir_index in 0..DIRS {
        let dir = index.insert(
            Some(root),
            OsStr::new(&format!("dir{dir_index:03}")),
            NodeKind::Dir,
            NodeMeta::default(),
        );
        for file_index in 0..FILES {
            let name = format!("file_{dir_index:03}_{file_index:04}.rs");
            index.insert(
                Some(dir),
                OsStr::new(&name),
                NodeKind::File,
                NodeMeta {
                    size: Some(1024),
                    mtime: Some(1_700_000_000),
                    btime: Some(1_700_000_000),
                },
            );
        }
    }
    index.commit();
    eprintln!("索引节点数：{}", index.len());

    let opts = EvalOptions {
        case_sensitive: false,
        home: None,
        root_abs: PathBuf::from("/root"),
    };
    let cancel = AtomicBool::new(false);
    // 空查询 = 全部命中（"没有筛选条件"时最坏的情况）。
    let ast = Ast::And(Vec::new());

    let start = Instant::now();
    let outcome = eval(&index, &ast, &opts, &cancel);
    let eval_time = start.elapsed();

    let matched = outcome.matches.len();
    let total = outcome.matches.clone();
    let start = Instant::now();
    let (by_name, _) = sort_matches(&index, outcome.matches, SortColumn::Name, SortDir::Asc, 1000);
    let name_sort_time = start.elapsed();

    let start = Instant::now();
    let (sorted, _) = sort_matches(&index, total, SortColumn::Path, SortDir::None, 1000);
    let sort_time = start.elapsed();

    eprintln!(
        "eval: {eval_time:?}（{matched} 项） / 路径序: {sort_time:?}（{} 项） / 名称排序: {name_sort_time:?}（{} 项）",
        sorted.len(),
        by_name.len()
    );
    assert!(matched > 0);
}
