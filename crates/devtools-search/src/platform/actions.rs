//! 打开与在文件管理器中显示（§9：开/显示分开）。
//!
//! macOS 用 `open` / `open -R`；Windows 用 `explorer /select,` 与 ShellExecute（`cmd /c start`）。

use std::io;
use std::path::Path;

use devtools_search_core::ports::SystemActions;

/// 用系统命令实现的系统动作。
#[derive(Clone, Copy, Debug, Default)]
pub struct StdActions;

impl SystemActions for StdActions {
    fn open(&self, path: &Path) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(path)
                .spawn()
                .map(|_| ())
        }
        #[cfg(windows)]
        {
            // `cmd /c start "" <path>`：交给 ShellExecute，按关联程序打开。
            std::process::Command::new("cmd")
                .args(["/c", "start", ""])
                .arg(path)
                .spawn()
                .map(|_| ())
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            std::process::Command::new("xdg-open").arg(path).spawn().map(|_| ())
        }
    }

    fn reveal(&self, path: &Path) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open").arg("-R").arg(path).spawn().map(|_| ())
        }
        #[cfg(windows)]
        {
            // `/select,` 与路径之间不能有空格。
            let arg = format!("/select,{}", path.display());
            std::process::Command::new("explorer").arg(arg).spawn().map(|_| ())
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let parent = path.parent().unwrap_or(path);
            std::process::Command::new("xdg-open").arg(parent).spawn().map(|_| ())
        }
    }
}
