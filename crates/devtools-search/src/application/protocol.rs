//! Service 与 UI 之间的命令/事件协议（§7.10）。
//!
//! 命令全部由 UI 线程发出、由 Service 串行处理（唯一设置所有者）；事件由 Service 与
//! 后台任务发回，UI 每帧 `poll()` 拉取。事件带会话/查询代际，过期结果直接丢弃。

use std::path::PathBuf;

use devtools_search_core::{NodeKind, ResultId, RootId, SortColumn, SortDir};
use devtools_search_core::volumes::RootState;

use crate::settings::{SearchSettings, SearchWindowMode};

/// 一次根的状态快照。
#[derive(Clone, Debug)]
pub struct RootStatus {
    pub id: RootId,
    pub path: PathBuf,
    pub state: RootState,
    pub files: u32,
    pub dirs: u32,
    /// 快照是否已加载（数据可能滞后）。
    pub from_snapshot: bool,
}

/// 表格一行的数据。
#[derive(Clone, Debug)]
pub struct RowData {
    /// 行在结果集中的下标（用于对齐虚拟化窗口）。
    pub index: u32,
    /// 结果已失效（删除/排除）时的占位行（§7.1：不当作真实结果）。
    pub missing: bool,
    pub result: ResultId,
    pub name: String,
    /// 完整路径。
    pub path: String,
    /// 所在目录。
    pub parent: String,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    pub btime: Option<i64>,
    pub kind: NodeKind,
}

/// 搜索完成的汇总。
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub session: u64,
    pub generation: u64,
    pub total: usize,
    pub took_ms: u64,
    /// 查询编译/求值错误（语法错误等）。
    pub error: Option<String>,
    /// 提示（例如 infolder 指向未配置索引的目录）。
    pub notice: Option<String>,
    pub ids: Vec<ResultId>,
}

/// UI → Service 的命令。
#[derive(Clone, Debug)]
pub enum Command {
    /// 预热（按 lazy_start / 首次引导状态决定是否扫描）。
    WarmUp,
    /// 立即开始建立索引（首次引导选"现在建立"）。
    StartIndexing,
    /// 提交查询。
    Search {
        text: String,
        /// 入口带来的原始文本（字面短语，不解析布尔/过滤器，§4.5）。
        literal: bool,
        case_sensitive: bool,
        scope: Option<PathBuf>,
    },
    /// 取一屏的行数据（128 行一批）。
    LoadRows { start: u32, count: u32 },
    /// 切换排序。
    SetSort { column: SortColumn, dir: SortDir },
    /// 提交设置草稿（Service 校验后原子保存并应用）。
    ApplySettings { draft: Box<SearchSettings> },
    /// 添加索引根（走原生选择对话框）。
    PickFolder,
    RemoveRoot { root: RootId },
    RebuildRoot { root: RootId },
    RebuildAll,
    Open { path: PathBuf },
    Reveal { path: PathBuf },
    PauseIndex,
    ResumeIndex,
    ClearCache,
    ClearHistory,
    RememberSize { size: [f32; 2] },
    /// 搜索页显示/隐藏（`continue_when_hidden` 决定隐藏时是否继续建索引）。
    VisibilityChanged { visible: bool },
    SetMode { mode: SearchWindowMode },
    Shutdown,
}

/// Service → UI 的事件。
#[derive(Clone, Debug)]
pub enum Event {
    /// 设置已加载（含损坏隔离/只读通知）。
    SettingsLoaded {
        settings: Box<SearchSettings>,
        notice: Option<String>,
        read_only: bool,
    },
    /// 设置应用结果：成功时带生效后的设置，失败时带校验问题。
    SettingsApplied(Result<Box<SearchSettings>, String>),
    /// 根状态变化（扫描开始/完成、卷离线等）。
    Roots { roots: Vec<RootStatus> },
    /// 查询完成。
    SearchDone(SearchResult),
    /// 一屏行数据。
    Rows { start: u32, rows: Vec<RowData> },
    /// 结果集失效（revision 变化但不需要重跑查询，§7.7）。
    ResultsStale,
    /// 一次性提示（toast）。
    Notice(String),
    /// Service 已停止（线程退出/资源释放完成）。
    Stopped,
}
