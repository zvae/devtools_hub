//! devtools-search：文件搜索模块（界面、设置、索引与宿主接入）。
//!
//! 架构（§3.2）：ui → application → core / platform / settings。
//! - `core` 是独立 crate（devtools-search-core），纯逻辑、无 UI、无平台依赖；
//! - 本 crate 负责 application（服务/协议/运行时）、settings、platform（std 实现）与 ui；
//! - 与宿主的唯一接口是 [`HostServices`]（原生对话框、系统动作、图标、窗口控制），
//!   以及模块公开的 [`ui::SearchModule`]。

pub mod application;
pub mod platform;
pub mod settings;
pub mod ui;

pub use application::protocol::{RowData, RootStatus};
pub use application::service::{Lifecycle, Service, WindowPolicy};
pub use settings::{SearchSettings, SearchWindowMode, SETTINGS_VERSION};
pub use ui::{SearchModule, SearchOptions, SearchPoll, SearchUiOutput};

use std::io;
use std::path::{Path, PathBuf};

pub use devtools_search_core::ports::SystemModalGuard;
pub use devtools_search_core::volumes::RootState;

/// 原生文件夹选择对话框的用途：决定给用户看的提示文案。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderPurpose {
    /// 添加索引根：选择要纳入索引的目录。
    IndexRoot,
    /// “在目录中搜索”的范围：只影响当前查询，不建立索引。
    SearchScope,
}

impl FolderPurpose {
    /// 对话框里的提示文字。
    pub fn message(self) -> &'static str {
        match self {
            Self::IndexRoot => "选择要建立索引的文件夹",
            Self::SearchScope => "选择要搜索的文件夹",
        }
    }
}

/// 宿主平台层必须提供的原生能力（§4.1）。
///
/// 这些实现不属于模块：文件夹选择、系统模态期间的原生窗口状态、打开/显示、原生图标、
/// 窗口层级与拖动，都必须操作宿主窗口（§0 的唯一机制让渡）。
pub trait HostServices: Send + Sync {
    /// 打开原生文件夹选择对话框；取消返回 `Ok(None)`。
    ///
    /// `purpose` 只用于对话框文案：同一个选择器既用于添加索引根，也用于设置当前搜索范围。
    fn pick_folder(
        &self,
        initial: Option<&Path>,
        purpose: FolderPurpose,
    ) -> Result<Option<PathBuf>, String>;

    /// 系统模态对话框打开前调用；返回的 guard 在对话框关闭后必须及时 `end()`。
    /// 打不开（例如宿主窗口还没就绪）时返回 `None`，模块直接给出诊断（§7.10）。
    fn begin_system_modal(&self) -> Option<Box<dyn SystemModalGuard>>;

    /// 用系统默认方式打开。
    fn open(&self, path: &Path) -> io::Result<()>;

    /// 在文件管理器中显示并选中。
    fn reveal(&self, path: &Path) -> io::Result<()>;

    /// 原生文件图标（RGBA）。`None` 时界面画占位符。
    fn icon(&self, path: &Path) -> Option<(u32, u32, Vec<u8>)>;

    /// 搜索页的窗口层级/空格行为（macOS 层级 101、空格键交给搜索框）。
    fn set_search_space_behavior(&self, searching: bool);

    /// 让 UI 线程重绘（事件到达时唤醒）。
    fn request_repaint(&self);

    /// 顶栏拖动窗口。返回 `true` 表示平台层已经自己发起了拖动；
    /// 返回 `false` 时由模块自己发 `ViewportCommand::StartDrag`（Windows 走 winit）。
    fn start_window_drag(&self) -> bool;
}
