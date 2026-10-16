//! 安装更新：用下载好的 dmg 替换正在运行的 .app，再启动新版本。
//!
//! 1. `hdiutil attach` 挂载 dmg（不在访达中显示），找到里面的 .app；
//! 2. 核对 Bundle ID 与版本号，`codesign --verify` 检查包是否完整；当前版本用证书签名时，
//!    新版本必须满足同样的签名要求（同一张证书），否则系统会把它当作另一个应用，“辅助功能”权限失效；
//! 3. 用 `ditto` 复制到旧版本旁边（同一个卷，之后的改名是原子的），再把旧版本改名移开、新版本改名到原位置。
//!    正在运行的 .app 可以改名，已经打开的文件不受影响；移开的旧版本由新版本启动时删除；
//! 4. 当前进程退出后由一个 shell 脚本用 `open` 启动新版本：应用还在运行时 `open` 只会激活它。

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, ensure, Context, Result};

/// 正在运行的 .app（`…/DevTools Hub.app/Contents/MacOS/devtools-hub` 往上三级）。
pub fn install_target() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|err| format!("找不到当前程序：{err}"))?;
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
        .filter(|path| path.join("Contents/Info.plist").is_file())
        .ok_or("只有打包成 .app 的版本支持自动安装")?;
    // 从下载目录直接打开、带隔离属性的应用会被系统移到只读的随机位置运行（App Translocation）。
    if bundle.to_string_lossy().contains("/AppTranslocation/") {
        return Err("请先把应用移到“应用程序”文件夹".into());
    }
    Ok(bundle.to_path_buf())
}

/// 用 `package`（dmg）中的应用替换 `target`。`version` 是 Release 的版本号，用来核对安装包。
pub fn install(package: &Path, target: &Path, version: &str) -> Result<()> {
    let mount = package.with_extension("mount");
    fs::create_dir_all(&mount)?;
    run(Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(&mount)
        .arg(package))
    .context("挂载安装包失败")?;
    let result = install_from(&mount, target, version);
    // 无论成功与否都卸载。
    if run(Command::new("hdiutil").arg("detach").arg(&mount)).is_err() {
        let _ = run(Command::new("hdiutil")
            .args(["detach", "-force"])
            .arg(&mount));
    }
    result
}

fn install_from(mount: &Path, target: &Path, version: &str) -> Result<()> {
    let source = fs::read_dir(mount)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .context("安装包里没有找到应用")?;
    // 核对是同一个应用、版本也对得上，避免装错。
    let current_id = bundle_value(target, "CFBundleIdentifier")?;
    let new_id = bundle_value(&source, "CFBundleIdentifier")?;
    ensure!(
        new_id == current_id,
        "安装包中的应用（{new_id}）与当前应用（{current_id}）不一致"
    );
    let new_version = bundle_value(&source, "CFBundleShortVersionString")?;
    ensure!(
        new_version == version,
        "安装包的版本（{new_version}）与 Release（{version}）不一致"
    );
    run(Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&source))
    .context("安装包中的应用签名无效")?;
    if let Some(requirement) = designated_requirement(target)? {
        run(Command::new("codesign")
            .args(["--verify", "--strict", "--test-requirement"])
            .arg(format!("={requirement}"))
            .arg(&source))
        .context("安装包的签名证书与当前应用不一致，装上后需要重新授予“辅助功能”权限")?;
    }

    let staged = sibling(target, "new")?;
    let old = sibling(target, "old")?;
    let _ = fs::remove_dir_all(&staged);
    let _ = fs::remove_dir_all(&old);
    run(Command::new("ditto").arg(&source).arg(&staged)).with_context(|| {
        format!(
            "复制新版本失败，请确认对 {} 有写入权限",
            target.parent().unwrap_or(target).display()
        )
    })?;
    // 下载的文件本来就不带隔离属性，这里保险起见再清一次，免得启动时被 Gatekeeper 拦下。
    let _ = run(Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&staged));
    if let Err(err) = fs::rename(target, &old) {
        let _ = fs::remove_dir_all(&staged);
        return Err(err).context("移开旧版本失败");
    }
    if let Err(err) = fs::rename(&staged, target) {
        let _ = fs::rename(&old, target);
        let _ = fs::remove_dir_all(&staged);
        return Err(err).context("放入新版本失败");
    }
    Ok(())
}

/// 等当前进程退出后启动 `target`，带上 `--after-update` 和 `args`。调用后当前进程应该立即退出。
pub fn relaunch(target: &Path, args: &[OsString]) -> Result<()> {
    // 最多等 20 秒；`--after-update` 让新版本在拿不到单实例锁时稍等片刻。
    // `open` 启动的应用不继承环境变量，参数要显式传过去。
    const SCRIPT: &str = r#"pid=$1 app=$2
shift 2
i=0
while kill -0 "$pid" 2>/dev/null && [ $i -lt 100 ]; do sleep 0.2; i=$((i + 1)); done
exec open "$app" --args --after-update "$@""#;
    Command::new("/bin/sh")
        .args(["-c", SCRIPT, "sh"])
        .arg(std::process::id().to_string())
        .arg(target)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("启动新版本失败")?;
    Ok(())
}

/// 新版本启动时删除上次更新移开的旧版本。
pub fn cleanup() {
    let Ok(target) = install_target() else {
        return;
    };
    for suffix in ["old", "new"] {
        if let Ok(path) = sibling(&target, suffix) {
            if path.exists() {
                match fs::remove_dir_all(&path) {
                    Ok(()) => tracing::info!("已删除 {}", path.display()),
                    Err(err) => tracing::warn!("删除 {} 失败：{err}", path.display()),
                }
            }
        }
    }
}

/// 更新后系统权限是否会失效：临时签名时系统按代码哈希识别应用，每个版本都不同。
pub fn permission_lost_on_update() -> bool {
    install_target()
        .ok()
        .and_then(|bundle| designated_requirement(&bundle).ok())
        .is_none_or(|requirement| requirement.is_none())
}

/// 应用的签名要求（designated requirement），例如
/// `identifier "dev.devtoolshub.app" and certificate leaf = H"…"`。
/// 临时签名没有可以跨版本使用的要求（只有本次构建的代码哈希），返回 `None`。
fn designated_requirement(bundle: &Path) -> Result<Option<String>> {
    let output = Command::new("codesign")
        .args(["--display", "--requirements", "-"])
        .arg(bundle)
        .output()
        .context("运行 codesign 失败")?;
    ensure!(
        output.status.success(),
        "读取 {} 的签名失败",
        bundle.display()
    );
    // 不同系统版本输出到标准输出或标准错误，两个都看。
    let text = [output.stdout, output.stderr].concat();
    Ok(parse_requirement(&String::from_utf8_lossy(&text)))
}

/// 从 `codesign --display --requirements -` 的输出中取出签名要求。
/// 临时签名输出的是注释掉的隐含要求：`# designated => cdhash H"…"`。
fn parse_requirement(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("designated => "))
        .filter(|requirement| !requirement.starts_with("cdhash "))
        .map(str::to_owned)
}

/// `…/DevTools Hub.app` → `…/.DevTools Hub.app.<suffix>`（隐藏文件，不会出现在“应用程序”里）。
fn sibling(target: &Path, suffix: &str) -> Result<PathBuf> {
    let name = target.file_name().context("无效的应用路径")?;
    Ok(target.with_file_name(format!(".{}.{suffix}", name.to_string_lossy())))
}

/// 读取 .app 的 Info.plist 中的一个字符串。
fn bundle_value(bundle: &Path, key: &str) -> Result<String> {
    let output = Command::new("plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .context("运行 plutil 失败")?;
    ensure!(
        output.status.success(),
        "读取 {} 的 {key} 失败",
        bundle.display()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// 运行命令，失败时把标准错误带进错误信息。
fn run(command: &mut Command) -> Result<()> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("运行 {:?} 失败", command.get_program()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        match stderr.trim() {
            "" => bail!("{:?} 失败（{}）", command.get_program(), output.status),
            message => bail!("{message}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_designated_requirement() {
        let signed = "Executable=/Applications/DevTools Hub.app/Contents/MacOS/devtools-hub\n\
                      designated => identifier \"dev.devtoolshub.app\" and certificate leaf = H\"39eb\"\n";
        assert_eq!(
            parse_requirement(signed).as_deref(),
            Some(r#"identifier "dev.devtoolshub.app" and certificate leaf = H"39eb""#)
        );
        let ad_hoc = "Executable=/tmp/a\n# designated => cdhash H\"2ce458c3\"\n";
        assert_eq!(parse_requirement(ad_hoc), None);
        assert_eq!(parse_requirement(""), None);
    }

    #[test]
    fn ad_hoc_bundle_has_no_stable_requirement() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Test.app");
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        fs::copy("/usr/bin/true", bundle.join("Contents/MacOS/test")).unwrap();
        fs::write(
            bundle.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.devtoolshub.test</string>
<key>CFBundleExecutable</key><string>test</string>
</dict></plist>"#,
        )
        .unwrap();
        run(Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&bundle))
        .unwrap();
        assert_eq!(designated_requirement(&bundle).unwrap(), None);
        assert_eq!(
            bundle_value(&bundle, "CFBundleIdentifier").unwrap(),
            "dev.devtoolshub.test"
        );
    }
}
