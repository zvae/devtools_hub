//! 安装更新：用下载好的 exe 替换正在运行的 exe，再启动新版本。
//!
//! 发布的是单个 exe。先在同目录暂存并同步新版本，再把运行中的 exe 改名为 `<名称>.old.exe`，
//! 将暂存文件移到正式路径。备份保留可执行扩展名，替换中断后可直接运行它恢复原路径。
//! 开机启动的注册表项指向的路径不变，备份由成功启动的新版本删除。
//! 自己下载的文件没有“来自网络”的标记（Zone.Identifier），启动时不会弹出 SmartScreen。

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};

/// 正在运行的 exe。
pub fn install_target() -> Result<PathBuf, String> {
    static RECOVERED_TARGET: OnceLock<PathBuf> = OnceLock::new();
    if let Some(target) = RECOVERED_TARGET.get() {
        return Ok(target.clone());
    }
    let exe = std::env::current_exe().map_err(|err| format!("找不到当前程序：{err}"))?;
    // 兼容旧版 .exe.old 备份；新版 .old.exe 可以直接双击运行。
    let name = exe.to_string_lossy().to_ascii_lowercase();
    let recovered_target = if name.ends_with(".old.exe") {
        Some(exe.with_extension("").with_extension("exe"))
    } else if name.ends_with(".exe.old") {
        Some(exe.with_extension(""))
    } else {
        None
    };
    if let Some(target) = recovered_target {
        if !target.exists() {
            fs::rename(&exe, &target).map_err(|err| format!("恢复旧版本失败：{err}"))?;
            // GetModuleFileName 可能仍返回加载时的备份路径。
            let _ = RECOVERED_TARGET.set(target.clone());
            return Ok(target);
        }
        return Err("请运行已安装的 exe，不能从更新备份再次更新".into());
    }
    Ok(exe)
}

/// 用 `package`（新版本的 exe）替换 `target`。
pub fn install(package: &Path, target: &Path, _version: &str) -> Result<()> {
    let old = old_path(target);
    let parent = target.parent().context("无效的安装目录")?;
    // 先在同卷写完、同步新版本，正式路径上永远不会出现复制到一半的 exe。
    let staged = tempfile::Builder::new()
        .prefix(".devtools-update-")
        .suffix(".exe")
        .tempfile_in(parent)?;
    fs::copy(package, staged.path()).context("暂存新版本失败")?;
    staged.as_file().sync_all().context("同步新版本失败")?;
    match fs::remove_file(&old) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("清理上次更新备份失败"),
    }
    fs::rename(target, &old).with_context(|| {
        format!(
            "移开旧版本失败，请确认对 {} 有写入权限",
            target.parent().unwrap_or(target).display()
        )
    })?;
    if let Err(error) = staged.persist_noclobber(target) {
        if let Err(rollback) = fs::rename(&old, target) {
            bail!(
                "安装失败：{}；恢复旧版本也失败：{rollback}。旧版本保留在 {}，可运行它恢复",
                error.error,
                old.display()
            );
        }
        return Err(error.error).context("放入新版本失败，已恢复旧版本");
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

/// 更新后系统权限是否会失效。Windows 上模拟按键、鼠标钩子都不需要授权。
pub fn permission_lost_on_update() -> bool {
    false
}

/// 新版本启动时删除上次更新移开的旧版本。
pub fn cleanup() {
    let Ok(target) = install_target() else {
        return;
    };
    let mut legacy = target.as_os_str().to_owned();
    legacy.push(".old");
    for old in [old_path(&target), PathBuf::from(legacy)] {
        if old.exists() {
            match fs::remove_file(&old) {
                Ok(()) => tracing::info!("已删除 {}", old.display()),
                Err(err) => tracing::warn!("删除 {} 失败：{err}", old.display()),
            }
        }
    }
}

/// `devtools-hub.exe` → `devtools-hub.old.exe`
fn old_path(target: &Path) -> PathBuf {
    target.with_extension("old.exe")
}
