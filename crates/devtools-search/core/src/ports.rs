//! 平台能力端口（§0、§3.2、§8、§9）。
//!
//! 只有定义，没有实现；core 不 import 平台 API。core 识别的唯一类型是：
//! FsSource / Watcher / Volume / Icon / Action，以及系统对话框与模态保护的 trait。

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use crate::model::{NodeMeta, VolumeId};

/// 目录项的类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    /// 符号链接 / reparse point（不递归，§7.3 交叉卷检查之外的保护）。
    Symlink,
    Other,
}

/// 扫描读取到的一个目录项（元数据在扫描时预取）。
#[derive(Debug)]
pub struct Entry {
    pub name: OsString,
    pub kind: EntryKind,
    pub meta: NodeMeta,
}

/// 文件系统来源（core 测试用 fake source；生产用平台的预取实现）。
pub trait FsSource: Send + Sync {
    /// 列出一个目录；`out` 由调用方复用（避免每目录分配）。
    /// 错误按目录上报（权限拒绝、IO 错误），不中断整棵扫描（§7.3）。
    fn list_dir(&self, path: &Path, out: &mut Vec<Entry>) -> io::Result<()>;

    /// Unix 下返回目录项所在设备号（`lstat` 的 `st_dev`），用于交叉卷检查；
    /// 不支持时返回 `None`（调用方跳过检查，§7.3 未决定的验证项）。
    fn device_of(&self, path: &Path) -> Option<u64>;
}

/// 卷信息来源（§7.5：挂载、拔出检测，根身份稳定化）。
pub trait VolumeSource: Send + Sync {
    /// 当前挂载的卷：volume_id + 挂载点。
    fn current_mounts(&self) -> Vec<VolumeMount>;

    /// 路径所在卷的身份（没有稳定身份时用显式标记的绝对路径兜底）。
    fn volume_id_of(&self, path: &Path) -> Option<VolumeId>;
}

/// 一个挂载点。
#[derive(Clone, Debug)]
pub struct VolumeMount {
    pub volume_id: VolumeId,
    pub mount_point: PathBuf,
}

/// 索引文件的原生图标与文件管理器动作（§3.2、§9）。
pub trait IconProvider: Send + Sync {
    /// 取原生图标（RGBA）；失败时 `None`（UI 画占位符）。
    fn icon_rgba(&self, path: &Path) -> Option<(u32, u32, Vec<u8>)>;
}

/// 系统动作：打开与在文件管理器中显示（§9：开/显示分开）。
pub trait SystemActions: Send + Sync {
    /// 用系统默认方式打开（双击等效）。
    fn open(&self, path: &Path) -> io::Result<()>;

    /// 在文件管理器中显示并选中（macOS：NSWorkspace activateFileViewerSelecting；
    /// Windows：Explorer `/select,`）。
    fn reveal(&self, path: &Path) -> io::Result<()>;
}

/// 文件系统变化监听（§8：FSEvents / ReadDirectoryChangesW；v1 尚无实现）。
pub trait Watcher: Send {
    /// 阻塞读取下一批变化；返回 `None` 表示结束。
    fn next_batch(&mut self) -> Option<Vec<PathBuf>>;
}

/// 原子写存储（§7.6：临时文件 → fsync → 原子替换）。
pub trait AtomicStorage: Send + Sync {
    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
}

/// 原生文件夹选择对话框。trait 由 core 定义，实现在宿主平台层（§0：唯一的机制让渡，
/// 必须操作宿主窗口的层级、应用激活和焦点恢复）。
pub trait FolderPicker: Send + Sync {
    /// 打开模态选文件夹；取消返回 `Ok(None)`；失败返回带诊断的错误（§5.3）。
    fn pick_folder(&self, initial: Option<&Path>) -> Result<Option<PathBuf>, String>;
}

/// 系统模态期间的原生窗口状态（§0、§7.10）。由 core 定义，宿主平台层实现。
pub trait SystemModalGuard: Send {
    /// 对话框关闭后调用；`restore_focus` 控制是否恢复之前前台应用的焦点。
    fn end(self: Box<Self>, restore_focus: bool);
}
