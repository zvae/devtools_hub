//! 宿主对搜索模块原生能力的实现（§4.2：唯一让渡给宿主的平台机制）。
//!
//! 文件夹选择、系统模态期间的原生窗口状态、打开/显示、窗口拖动与重绘唤醒都落在
//! 宿主平台层（`platform::{floating, panel, workspace}`），模块只依赖 trait。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use devtools_search::{HostServices, SystemModalGuard};

use crate::platform::{floating, panel, workspace};
use crate::shared::Shared;

/// 宿主实现。
pub struct HostBridge {
    shared: Arc<Shared>,
}

impl HostBridge {
    pub fn new(shared: Arc<Shared>) -> Arc<dyn HostServices> {
        Arc::new(Self { shared })
    }
}

/// 模态 guard：结束对话框后恢复层级与焦点。
struct ModalGuard;

impl SystemModalGuard for ModalGuard {
    fn end(self: Box<Self>, restore_focus: bool) {
        floating::end_system_modal(restore_focus);
    }
}

impl HostServices for HostBridge {
    fn pick_folder(&self, initial: Option<&Path>) -> Result<Option<PathBuf>, String> {
        floating::pick_folder(initial)
    }

    fn begin_system_modal(&self) -> Option<Box<dyn SystemModalGuard>> {
        floating::begin_system_modal().then(|| Box::new(ModalGuard) as Box<dyn SystemModalGuard>)
    }

    fn open(&self, path: &Path) -> io::Result<()> {
        workspace::open_path(path);
        Ok(())
    }

    fn reveal(&self, path: &Path) -> io::Result<()> {
        workspace::reveal_path(&path.to_string_lossy());
        Ok(())
    }

    fn icon(&self, path: &Path) -> Option<(u32, u32, Vec<u8>)> {
        // 32×32 的系统图标（按 18px 绘制，Retina 下更清晰）。
        crate::platform::icons::file_icon_rgba(path, 32)
    }

    fn set_search_space_behavior(&self, searching: bool) {
        panel::set_search_space_behavior(searching);
    }

    fn request_repaint(&self) {
        self.shared.request_repaint();
    }

    fn start_window_drag(&self) -> bool {
        panel::start_drag()
    }
}
