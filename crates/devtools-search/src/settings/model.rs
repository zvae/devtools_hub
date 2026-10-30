//! 搜索设置模型（§6.1：SearchSettings 各字段）。
//!
//! 序列化兼容要求：新增字段必须有默认值（`#[serde(default)]` 在结构体上），
//! 未知字段反序列化时不报错；枚举使用 snake_case，避免不可读的数字布局。
//! 类型边界：core 的 RootLocation/VolumeId 等纯类型直接复用，settings 不反向依赖 core 之外的层。

use std::path::PathBuf;

use devtools_search_core::{RootLocation, SortColumn, SortDir, VolumeId};
use serde::{Deserialize, Serialize};

/// 当前设置格式版本（§6.2：从 version 1 开始）。
pub const SETTINGS_VERSION: u32 = 1;

/// 一个索引根（`enabled` 与运行状态分开，§7.5）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RootConfig {
    pub location: RootLocation,
    pub enabled: bool,
}

/// 搜索窗口模式（§5.2：Floating 失焦可隐藏 / Pinned 失焦保留）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchWindowMode {
    #[default]
    Floating,
    Pinned,
}

/// 窗口偏好。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPrefs {
    pub size: [f32; 2],
    pub mode: SearchWindowMode,
}

impl Default for WindowPrefs {
    fn default() -> Self {
        Self {
            size: [1100.0, 680.0],
            mode: SearchWindowMode::default(),
        }
    }
}

/// 五列权重（§5.1：文件名 / 路径 / 大小 / 修改时间 / 创建时间）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColumnWidths {
    pub name: f32,
    pub path: f32,
    pub size: f32,
    pub modified: f32,
    pub created: f32,
}

impl Default for ColumnWidths {
    fn default() -> Self {
        Self {
            name: 260.0,
            path: 450.0,
            size: 100.0,
            modified: 155.0,
            created: 155.0,
        }
    }
}

/// 排序偏好。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSort {
    pub column: SortColumn,
    pub descending: bool,
}

impl Default for UiSort {
    fn default() -> Self {
        Self {
            column: SortColumn::Path,
            descending: false,
        }
    }
}

impl UiSort {
    /// 换算成 core 的排序方向（Path 列始终默认路径序）。
    pub fn dir(self) -> SortDir {
        if self.column == SortColumn::Path {
            SortDir::None
        } else if self.descending {
            SortDir::Desc
        } else {
            SortDir::Asc
        }
    }
}

/// UI 偏好。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    pub window: WindowPrefs,
    pub columns: ColumnWidths,
    pub sort: UiSort,
}

/// 一条历史记录（§5.4）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub query: String,
    /// 目录范围（可选）。
    pub scope: Option<PathBuf>,
    pub case_sensitive: bool,
    /// 提交时间（Unix 秒）。
    pub at: i64,
}

/// 保存的搜索（§6.1 保留字段；v1 界面不提供保存搜索）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedSearch {
    pub name: String,
    pub query: String,
    pub scope: Option<PathBuf>,
    pub case_sensitive: bool,
}

/// 搜索设置（version 1）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSettings {
    pub version: u32,
    pub roots: Vec<RootConfig>,
    pub exclude_patterns: Vec<String>,
    pub exclude_case_sensitive: bool,
    /// 卷相对格式的忽略路径（与根同一模型）。
    pub ignore_paths: Vec<RootLocation>,
    pub scan_app_bundles: bool,
    pub watch: bool,
    pub lazy_start: bool,
    pub continue_when_hidden: bool,
    pub index_paused: bool,
    pub persist_index: bool,
    pub debounce_ms: u32,
    pub default_case_sensitive: bool,
    pub show_all_on_empty: bool,
    pub max_results: u32,
    pub highlight: bool,
    pub history_enabled: bool,
    pub history_limit: u32,
    pub permission_onboarding_completed: bool,
    pub ui: UiPrefs,
    pub history: Vec<HistoryEntry>,
    pub saved_searches: Vec<SavedSearch>,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            roots: default_roots(),
            exclude_patterns: default_excludes(),
            exclude_case_sensitive: false,
            ignore_paths: Vec::new(),
            scan_app_bundles: false,
            watch: true,
            lazy_start: true,
            continue_when_hidden: false,
            index_paused: false,
            persist_index: true,
            debounce_ms: 100,
            default_case_sensitive: false,
            show_all_on_empty: true,
            max_results: 1000,
            highlight: true,
            history_enabled: true,
            history_limit: 50,
            permission_onboarding_completed: false,
            ui: UiPrefs::default(),
            history: Vec::new(),
            saved_searches: Vec::new(),
        }
    }
}

/// 默认根：用户主目录（卷身份未确定时用显式标记的绝对路径兜底，§7.5）。
fn default_roots() -> Vec<RootConfig> {
    let Some(home) = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()) else {
        return Vec::new();
    };
    vec![RootConfig {
        location: RootLocation {
            volume_id: VolumeId::fallback_absolute(&home),
            relative_path: String::new(),
            fallback_absolute_path: home,
        },
        enabled: true,
    }]
}

/// 默认排除规则（§7.4 的平台默认值）。
///
/// 语义（与 `scan::ExcludeSet` 一致，2026-10-30 确认）：**从根目录开始写完整路径才是根相对**。
/// - 单段规则（`node_modules/`、`Library/`、`*.log`）一律按任意层级匹配，结尾 `/` 只表示目录；
/// - 多段完整路径（`AppData/Local/Temp/`）或前导 `/`（`/Library/`）才按根相对路径匹配。
///
/// 因此默认表里的单段项会剪掉任意层级的同名目录；只想排除根下那一层时写成多段/带前导 `/`。
fn default_excludes() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        vec![
            "node_modules/".into(),
            "target/".into(),
            "DerivedData/".into(),
            ".git/".into(),
            "Library/".into(),
        ]
    }
    #[cfg(windows)]
    {
        vec![
            "node_modules/".into(),
            "target/".into(),
            ".git/".into(),
            "AppData/Local/Temp/".into(),
            "AppData/Local/Packages/".into(),
            // 常见包管理器缓存。
            "AppData/Local/pnpm-store/".into(),
            "AppData/Local/Yarn/Cache/".into(),
            "AppData/Roaming/npm-cache/".into(),
            "AppData/Local/NuGet/Cache/".into(),
            "AppData/Local/go-build/".into(),
        ]
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        vec!["node_modules/".into(), "target/".into(), ".git/".into()]
    }
}

impl SearchSettings {
    /// 校验（§6.2：范围、去重、嵌套根、未知列宽/方向、非法模式）。
    /// 返回全部问题；空向量表示可保存。
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.version != SETTINGS_VERSION {
            problems.push(format!("未知设置版本：{}", self.version));
        }
        if !(1..=2000).contains(&self.debounce_ms) {
            problems.push("防抖间隔必须在 1–2000 毫秒之间".into());
        }
        if self.history_limit > 10_000 {
            problems.push("历史上限不能超过 10000".into());
        }
        for (name, value) in [
            ("文件名", self.ui.columns.name),
            ("路径", self.ui.columns.path),
            ("大小", self.ui.columns.size),
            ("修改时间", self.ui.columns.modified),
            ("创建时间", self.ui.columns.created),
        ] {
            if !value.is_finite() || value <= 0.0 {
                problems.push(format!("列宽无效：{name} 列必须为正数"));
            }
        }
        if !self.ui.window.size[0].is_finite()
            || !self.ui.window.size[1].is_finite()
            || self.ui.window.size[0] < 800.0
            || self.ui.window.size[1] < 480.0
        {
            problems.push("搜索窗口尺寸无效（最小 800×480）".into());
        }
        // 排除规则可编译（含 `..` 拒绝、非法 glob 报错）。
        if let Err(err) = devtools_search_core::scan::ExcludeSet::compile(
            &self.exclude_patterns,
            self.exclude_case_sensitive,
        ) {
            problems.push(err);
        }
        // 根冲突（去重 + 嵌套）。
        for (i, root) in self.roots.iter().enumerate() {
            let others: Vec<RootLocation> = self
                .roots
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, r)| r.location.clone())
                .collect();
            if let Err(conflict) =
                devtools_search_core::volumes::validate_new_root(&root.location, &others)
            {
                problems.push(format!("索引根 {}：{conflict}", root.location.fallback_absolute_path.display()));
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_design() {
        let s = SearchSettings::default();
        assert_eq!(s.version, 1);
        assert_eq!(s.debounce_ms, 100);
        assert_eq!(s.max_results, 1000);
        assert_eq!(s.history_limit, 50);
        assert_eq!(s.ui.window.size, [1100.0, 680.0]);
        assert_eq!(s.ui.columns.name, 260.0);
        assert_eq!(s.ui.columns.path, 450.0);
        assert_eq!(s.ui.columns.size, 100.0);
        assert_eq!(s.ui.columns.modified, 155.0);
        assert_eq!(s.ui.columns.created, 155.0);
        assert!(s.lazy_start);
        assert!(s.watch);
        assert!(!s.continue_when_hidden);
        assert!(!s.default_case_sensitive);
        assert!(s.show_all_on_empty);
        assert!(s.highlight);
        assert!(s.history_enabled);
        assert!(s.persist_index);
        assert_eq!(s.ui.window.mode, SearchWindowMode::Floating);
        assert!(s.validate().is_empty(), "默认设置必须可保存：{:?}", s.validate());
        #[cfg(target_os = "macos")]
        assert_eq!(
            s.exclude_patterns,
            ["node_modules/", "target/", "DerivedData/", ".git/", "Library/"]
        );
    }

    #[test]
    fn missing_fields_use_defaults() {
        let json = r#"{"version":1,"debounce_ms":250}"#;
        let s: SearchSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.debounce_ms, 250);
        assert_eq!(s.max_results, 1000);
        assert_eq!(s.ui.columns.path, 450.0);
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        let json = r#"{"version":1,"some_future_field":{"a":1},"debounce_ms":50}"#;
        let s: SearchSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.debounce_ms, 50);
    }

    #[test]
    fn enum_layout_is_snake_case() {
        let s = SearchSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"mode\":\"floating\""), "{json}");
        assert!(json.contains("\"column\":\"path\""), "{json}");
        assert!(json.contains("\"version\":1"));
    }

    #[test]
    fn validation_rejects_bad_values() {
        let mut s = SearchSettings::default();
        s.debounce_ms = 0;
        s.ui.columns.name = -1.0;
        s.exclude_patterns.push("../bad".into());
        let problems = s.validate();
        assert!(problems.iter().any(|p| p.contains("防抖")));
        assert!(problems.iter().any(|p| p.contains("列宽")));
        assert!(problems.iter().any(|p| p.contains("..")));
        // 嵌套根。
        let mut s = SearchSettings::default();
        let home = s.roots[0].location.clone();
        let mut nested = home.clone();
        nested.relative_path = "Documents".into();
        nested.fallback_absolute_path = home.fallback_absolute_path.join("Documents");
        s.roots.push(RootConfig { location: nested, enabled: true });
        assert!(s.validate().iter().any(|p| p.contains("嵌套")));
    }
}
