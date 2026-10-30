//! 扫描与排除规则（§7.3、§7.4）。
//!
//! - 扫描时预取元数据（大小/修改/创建时间在遍历项里给出），查询阶段禁止按候选逐个 stat；
//! - 交叉卷检查：Unix 用 `lstat` 的 `st_dev`，只同设备号递归；符号链接/reparse 目录不递归；
//! - 排除规则（2026-10-30 确认的语义）：**要从根目录写起才是根相对**——
//!   单段规则（`node_modules`、`node_modules/`、`Library/`、`*.log`）一律按任意层级匹配，
//!   结尾斜杠只表示"只匹配目录"；多段完整路径（`AppData/Local/Temp/`、`a/b`）或前导 `/`
//!   （`/Library/`）才是根相对；`**/build/**` 子树剪枝；拒绝 `..`；大小写由配置决定；
//! - 命中排除的目录不 walk 内部；节点本身保留为 `Excluded` 状态（解除规则无需重建即可恢复，§6.3）。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use globset::{GlobBuilder, GlobSetBuilder};

use crate::index::RootIndex;
use crate::model::{NodeKind, NodeMeta};
use crate::ports::{Entry, EntryKind, FsSource};

/// 编译后的排除规则集（对"根相对路径字符串"匹配）。
pub struct ExcludeSet {
    /// 命中目录 → 剪枝（该目录及后代都排除）。
    dir_prune: globset::GlobSet,
    /// 命中文件 → 排除。
    file_match: globset::GlobSet,
    pub empty: bool,
}

impl ExcludeSet {
    /// 编译规则。非法规则（含 `..`、空规则）整体报错。
    pub fn compile(rules: &[String], case_sensitive: bool) -> Result<Self, String> {
        let mut dir = GlobSetBuilder::new();
        let mut file = GlobSetBuilder::new();
        let mut any = false;
        for raw in rules {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            if raw.split(['/', '\\']).any(|seg| seg == "..") {
                return Err(format!("排除规则不能包含 `..`：{raw}"));
            }
            // 前导 `/` 显式表示"从根目录开始"。
            let (anchored_root, rest) = match raw.strip_prefix('/') {
                Some(rest) => (true, rest),
                None => (false, raw),
            };
            if rest.is_empty() {
                return Err(format!("排除规则无效：{raw}"));
            }
            let dir_only = rest.ends_with('/');
            let body = rest.trim_end_matches('/');
            if body.is_empty() {
                return Err(format!("排除规则无效：{raw}"));
            }
            any = true;
            // 根相对 = 显式前导 `/` 或从根写起的完整多段路径（已去掉结尾斜杠后仍含 `/`）。
            // 只有单段的规则一律"任意层级"匹配。
            let root_relative = anchored_root || body.contains('/');
            if root_relative {
                if let Some(prefix) = body.strip_suffix("/**") {
                    // `**/build/**`：目录剪枝 + 内容匹配。
                    dir.add(glob(prefix, case_sensitive)?);
                    file.add(glob(body, case_sensitive)?);
                } else if dir_only {
                    // `AppData/Local/Temp/`：根相对、只匹配目录。
                    dir.add(glob(body, case_sensitive)?);
                    // 供事件路径检查用：目录下的内容同样属于排除子树。
                    file.add(glob(&format!("{body}/**"), case_sensitive)?);
                } else {
                    // `a/b`：根相对路径，文件与目录都可命中。
                    dir.add(glob(body, case_sensitive)?);
                    file.add(glob(body, case_sensitive)?);
                }
            } else if dir_only {
                // `node_modules/`、`Library/`：任意层级、只匹配目录（结尾斜杠不改变层级语义）。
                let pattern = format!("**/{body}");
                dir.add(glob(&pattern, case_sensitive)?);
                // 供事件路径检查用：目录下的内容同样属于排除子树。
                file.add(glob(&format!("{pattern}/**"), case_sensitive)?);
            } else {
                // `node_modules`、`*.log`：任意层级同名，文件与目录都可命中。
                let pattern = format!("**/{body}");
                dir.add(glob(&pattern, case_sensitive)?);
                file.add(glob(&pattern, case_sensitive)?);
            }
        }
        Ok(Self {
            dir_prune: dir.build().map_err(|e| e.to_string())?,
            file_match: file.build().map_err(|e| e.to_string())?,
            empty: !any,
        })
    }

    /// 该根相对路径的目录是否应剪枝。
    pub fn is_excluded_dir(&self, rel: &str) -> bool {
        !self.empty && self.dir_prune.is_match(rel)
    }

    /// 该根相对路径的文件是否应排除。
    pub fn is_excluded_file(&self, rel: &str) -> bool {
        !self.empty && self.file_match.is_match(rel)
    }

    /// 路径（文件或目录）是否落在排除子树内（供事件更新用）。
    pub fn is_excluded_path(&self, rel: &str) -> bool {
        self.is_excluded_dir(rel) || self.is_excluded_file(rel)
    }
}

fn glob(pattern: &str, case_sensitive: bool) -> Result<globset::Glob, String> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|e| format!("排除规则 `{pattern}` 无效：{e}"))
}

/// 扫描选项。
pub struct ScanOptions {
    pub excludes: ExcludeSet,
    /// `.app` 是否作为目录扫描内部；false 时作为文件节点（§7.3）。
    pub scan_app_bundles: bool,
    /// 根所在设备号（Unix）；目录项设备号不同则不递归（§7.3 交叉卷检查）。
    pub root_device: Option<u64>,
}

/// 扫描统计。
#[derive(Default, Debug)]
pub struct ScanStats {
    pub files: u32,
    pub dirs: u32,
    /// 权限拒绝 / IO 错误的目录数（§7.3）。
    pub denied: u32,
    pub excluded: u32,
    pub symlinks: u32,
}

/// 扫描结果。
pub struct ScanOutcome {
    pub stats: ScanStats,
    /// 取消（部分索引由调用方丢弃）。
    pub cancelled: bool,
}

/// 扫描一个根：在 `index` 中构建完整节点表。成功时提交一次（重建派生数据）。
pub fn scan_root(
    source: &dyn FsSource,
    root_path: &Path,
    opts: &ScanOptions,
    cancel: &AtomicBool,
    index: &mut RootIndex,
) -> ScanOutcome {
    let mut stats = ScanStats::default();
    let root_name: OsString = root_path
        .file_name()
        .unwrap_or_else(|| root_path.as_os_str())
        .to_os_string();
    let root = index.insert(None, &root_name, NodeKind::Dir, NodeMeta::default());
    stats.dirs = 1;

    // 工作栈：(绝对路径, 节点, 根相对路径)。
    let mut stack: Vec<(PathBuf, crate::model::NodeId, String)> =
        vec![(root_path.to_path_buf(), root, String::new())];
    let mut entries: Vec<Entry> = Vec::new();
    let mut scanned_dirs = 0u32;

    while let Some((dir_path, dir_node, rel)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return ScanOutcome { stats, cancelled: true };
        }
        entries.clear();
        if source.list_dir(&dir_path, &mut entries).is_err() {
            stats.denied += 1;
            continue;
        }
        scanned_dirs += 1;
        for entry in &entries {
            if (stats.files + stats.dirs) % 2048 == 0 && cancel.load(Ordering::Relaxed) {
                return ScanOutcome { stats, cancelled: true };
            }
            let rel_path = if rel.is_empty() {
                entry.name.to_string_lossy().into_owned()
            } else {
                format!("{rel}/{}", entry.name.to_string_lossy())
            };
            let abs_path = dir_path.join(&entry.name);
            match entry.kind {
                EntryKind::Dir => {
                    let excluded = opts.excludes.is_excluded_dir(&rel_path);
                    let is_bundle = !opts.scan_app_bundles && is_app_bundle(&entry.name);
                    // 交叉卷：设备号不同则不递归（§7.3）。
                    let cross_volume = opts
                        .root_device
                        .is_some_and(|root_dev| source.device_of(&abs_path) != Some(root_dev));
                    if is_bundle {
                        // 包整体作为文件节点（§7.3）。
                        index.insert(
                            Some(dir_node),
                            &entry.name,
                            NodeKind::File,
                            entry.meta,
                        );
                        stats.files += 1;
                        continue;
                    }
                    let node = index.insert(Some(dir_node), &entry.name, NodeKind::Dir, entry.meta);
                    stats.dirs += 1;
                    if excluded {
                        index.set_excluded(node, true);
                        stats.excluded += 1;
                        // 命中排除项不 walk 内部（§7.4）。
                    } else if !cross_volume {
                        stack.push((abs_path, node, rel_path));
                    }
                }
                EntryKind::File | EntryKind::Other => {
                    let node = index.insert(Some(dir_node), &entry.name, NodeKind::File, entry.meta);
                    stats.files += 1;
                    if opts.excludes.is_excluded_file(&rel_path) {
                        index.set_excluded(node, true);
                        stats.excluded += 1;
                    }
                }
                EntryKind::Symlink => {
                    // 符号链接/reparse 目录不递归（避免环与交叉卷，§7.3）。
                    let node = index.insert(Some(dir_node), &entry.name, NodeKind::File, entry.meta);
                    stats.files += 1;
                    stats.symlinks += 1;
                    let _ = node;
                }
            }
        }
    }
    let _ = scanned_dirs;
    index.commit();
    ScanOutcome { stats, cancelled: false }
}

/// `.app` 包名（大小写不敏感；§7.3）。
fn is_app_bundle(name: &OsStr) -> bool {
    name.to_string_lossy().to_ascii_lowercase().ends_with(".app")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusion_rules_use_root_path_or_any_level() {
        let set = ExcludeSet::compile(
            &[
                "node_modules/".into(),
                ".git/".into(),
                "*.log".into(),
                "Library/".into(),
                "/TopOnly/".into(),
                "AppData/Local/Temp/".into(),
                "**/build/**".into(),
            ],
            false,
        )
        .unwrap();
        // 单段：任意层级同名（文件与目录）。
        assert!(set.is_excluded_dir("node_modules"));
        assert!(set.is_excluded_dir("a/b/node_modules"));
        assert!(set.is_excluded_dir("repo/.git"));
        assert!(set.is_excluded_file("a/b/c.log"));
        assert!(!set.is_excluded_file("a/b/c.txt"));
        // 单段 + 结尾斜杠：仍然任意层级，只匹配目录。
        assert!(set.is_excluded_dir("Library"));
        assert!(set.is_excluded_dir("deep/Library"));
        assert!(!set.is_excluded_file("Library"));
        // 前导 `/`：从根目录开始，只命中顶层。
        assert!(set.is_excluded_dir("TopOnly"));
        assert!(!set.is_excluded_dir("deep/TopOnly"));
        // 多段完整路径：从根写起才是根相对。
        assert!(set.is_excluded_dir("AppData/Local/Temp"));
        assert!(!set.is_excluded_dir("x/AppData/Local/Temp"));
        assert!(set.is_excluded_file("AppData/Local/Temp/a.tmp"));
        // **/build/**：build 目录剪枝（命中则整棵不进入）。
        assert!(set.is_excluded_dir("build"));
        assert!(set.is_excluded_dir("a/build"));
        assert!(set.is_excluded_file("build/out.bin"));
        assert!(set.is_excluded_file("a/build/out.bin"));
    }

    #[test]
    fn single_segment_matches_any_level_slash_means_dir_only() {
        let bare = ExcludeSet::compile(&["cache".into()], false).unwrap();
        assert!(bare.is_excluded_dir("cache"));
        assert!(bare.is_excluded_dir("deep/cache"));
        assert!(bare.is_excluded_file("deep/cache"));
        let dir_only = ExcludeSet::compile(&["cache/".into()], false).unwrap();
        assert!(dir_only.is_excluded_dir("deep/cache"));
        assert!(!dir_only.is_excluded_file("deep/cache"));
        // 目录下的内容属于排除子树（供事件路径检查）。
        assert!(dir_only.is_excluded_path("deep/cache/a.bin"));
    }

    #[test]
    fn root_anchored_rules_need_full_path_from_root() {
        let set = ExcludeSet::compile(&["/Library/".into()], false).unwrap();
        assert!(set.is_excluded_dir("Library"));
        assert!(!set.is_excluded_dir("deep/Library"));
        // 前导 `/` 与多段路径等价。
        let multi = ExcludeSet::compile(&["a/b/".into()], false).unwrap();
        assert!(multi.is_excluded_dir("a/b"));
        assert!(!multi.is_excluded_dir("x/a/b"));
    }

    #[test]
    fn rejects_parent_references() {
        assert!(ExcludeSet::compile(&["../outside".into()], false).is_err());
        assert!(ExcludeSet::compile(&["a/../b".into()], false).is_err());
        assert!(ExcludeSet::compile(&["good".into()], false).is_ok());
    }

    #[test]
    fn case_sensitivity_follows_config() {
        let sensitive = ExcludeSet::compile(&["Secret".into()], true).unwrap();
        assert!(sensitive.is_excluded_file("Secret"));
        assert!(!sensitive.is_excluded_file("secret"));
        let insensitive = ExcludeSet::compile(&["Secret".into()], false).unwrap();
        assert!(insensitive.is_excluded_file("secret"));
    }

    #[test]
    fn wildcard_matcher_handles_utf8() {
        use crate::query::wildcard_match;
        assert!(wildcard_match("*.txt", "报告.txt"));
        assert!(wildcard_match("报告*", "报告2026.txt"));
        assert!(!wildcard_match("*.txt", "报告.md"));
        assert!(wildcard_match("a?c", "a中c"));
        assert!(!wildcard_match("a?c", "ac"));
    }
}
