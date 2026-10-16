//! 安装更新：用下载好的 exe 替换正在运行的 exe，再启动新版本。
//!
//! 发布的是单个 exe。正在运行的 exe 不能覆盖或删除，但可以改名：先把它改名为 `<名称>.exe.old`，
//! 再把新版本写到原来的路径；开机启动的注册表项指向的路径不变。改名移开的旧版本由新版本启动时删除。
//! 自己下载的文件没有“来自网络”的标记（Zone.Identifier），启动时不会弹出 SmartScreen。

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// 正在运行的 exe。
pub fn install_target() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|err| format!("找不到当前程序：{err}"))
}

/// 用 `package`（新版本的 exe）替换 `target`。
pub fn install(package: &Path, target: &Path, _version: &str) -> Result<()> {
    let old = old_path(target);
    let _ = fs::remove_file(&old);
    fs::rename(target, &old).with_context(|| {
        format!(
            "移开旧版本失败，请确认对 {} 有写入权限",
            target.parent().unwrap_or(target).display()
        )
    })?;
    if let Err(err) = fs::copy(package, target) {
        let _ = fs::remove_file(target);
        let _ = fs::rename(&old, target);
        return Err(err).context("写入新版本失败");
    }
    Ok(())
}

/// 启动新版本，带上 `--after-update` 和 `args`。调用后当前进程应该立即退出；
/// 新版本会等当前进程释放单实例锁。
pub fn relaunch(target: &Path, args: &[OsString]) -> Result<()> {
    Command::new(target)
        .arg("--after-update")
        .args(args)
        .spawn()
        .context("启动新版本失败")?;
    Ok(())
}

/// 新版本启动时删除上次更新移开的旧版本。
pub fn cleanup() {
    let Ok(target) = install_target() else {
        return;
    };
    let old = old_path(&target);
    if old.exists() {
        match fs::remove_file(&old) {
            Ok(()) => tracing::info!("已删除 {}", old.display()),
            Err(err) => tracing::warn!("删除 {} 失败：{err}", old.display()),
        }
    }
}

/// `devtools-hub.exe` → `devtools-hub.exe.old`
fn old_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}
