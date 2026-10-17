//! 开机启动。
//!
//! PasteNow 在 `Contents/Library/LoginItems/PasteNowHelper.app` 中放了一个辅助应用，
//! 通过 ServiceManagement 注册为登录项（这要求应用以 .app 包形式签名分发）。
//! 这里用更通用的 LaunchAgent：在 `~/Library/LaunchAgents` 写入 plist，
//! 无论是 `cargo run` 出来的可执行文件还是打包后的 .app 都能工作。

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

/// LaunchAgent 的标签，也是 plist 的文件名。
const LABEL: &str = "dev.devtoolshub.clipboard";

/// `~/Library/LaunchAgents/<LABEL>.plist`
fn plist_path() -> Result<PathBuf> {
    let home = directories::BaseDirs::new().context("找不到用户目录")?;
    Ok(home
        .home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

/// 是否已设置登录时启动（plist 存在）。
pub fn is_enabled() -> bool {
    plist_path().map(|path| path.exists()).unwrap_or(false)
}

/// 开启时写入指向当前可执行文件的 plist，关闭时删除它。
pub fn set_enabled(enabled: bool) -> Result<()> {
    let path = plist_path()?;
    if !enabled {
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("删除 {} 失败", path.display()))?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe().context("无法获取可执行文件路径")?;
    let exe = xml_escape(&exe.to_string_lossy());
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, plist).with_context(|| format!("写入 {} 失败", path.display()))
}

/// 转义写进 plist 的文本（路径里可能有 `&` 等字符）。
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
