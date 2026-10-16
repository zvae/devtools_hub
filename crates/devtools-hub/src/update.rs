//! 检查与安装更新：查询 GitHub Releases 上最新的正式版本，比当前版本新时提示升级，并在应用内安装。
//!
//! - 发布流程（`.github/workflows/release.yml`）在 main 上生成 Release 草稿，手动发布后才会被检测到：
//!   接口 `releases/latest` 只返回已发布的正式版本，草稿和预发布都不算；
//! - 启动后稍等片刻自动检查一次，之后每天检查一次（可以在设置中关闭）；设置页底部可以手动检查；
//! - 安装：下载当前平台的安装包（核对大小与 GitHub 记录的 SHA-256），由 `platform::updater`
//!   替换正在运行的程序，再启动新版本并退出。用 `cargo run` 启动、Release 里没有对应的安装包
//!   或者安装失败时，改为在浏览器中下载。

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use ureq::config::ConfigBuilder;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::typestate::AgentScope;

use crate::platform::updater;
use crate::shared::Shared;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// 源代码仓库地址（来自 Cargo.toml 的 `repository`），Release 也发布在这里。
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// 启动后多久做第一次自动检查：开机自启时网络可能还没连上。
const STARTUP_DELAY: Duration = Duration::from_secs(10);
/// 自动检查的间隔。
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// 后台线程多久醒来看一次是否到期。按墙上时间判断，电脑睡眠过也能按时检查；检查失败时也按这个间隔重试。
const WAKE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const TIMEOUT: Duration = Duration::from_secs(20);
/// 下载安装包的总时长上限（ureq 没有单次读取的空闲超时，网络卡住时靠它结束）。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// 下载进度多久通知一次 UI。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// 不带前缀 `v` 的版本号，如 `0.2.0`。
    pub version: String,
    /// Release 页面（更新说明）。
    pub page_url: String,
    /// 当前平台的安装包。
    pub asset: Option<Asset>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// GitHub 记录的 SHA-256（小写十六进制）；比较早上传的附件没有。
    pub sha256: Option<String>,
}

impl Release {
    /// 浏览器下载的地址；Release 里没有当前平台的安装包时用 Release 页面。
    pub fn download_or_page(&self) -> &str {
        self.asset
            .as_ref()
            .map_or(&self.page_url, |asset| &asset.url)
    }
}

/// 上次检查的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// 还没有检查过。
    #[default]
    Unknown,
    UpToDate,
    Available(Release),
    /// 手动检查失败的原因（自动检查失败不显示，保留上次的结果）。
    Failed(String),
}

/// 应用内安装的进度。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    Downloading {
        received: u64,
        total: u64,
    },
    Installing,
    /// 新版本已经替换到位，等 UI 线程启动它并退出。
    Installed(PathBuf),
    Failed(String),
}

impl Install {
    /// 正在下载或安装（这时不能再开始一次）。
    pub fn in_progress(&self) -> bool {
        !matches!(self, Install::Failed(_))
    }
}

#[derive(Clone, Debug, Default)]
pub struct UpdateState {
    pub checking: bool,
    pub status: Status,
    /// 上次成功检查的时间。
    pub checked_at: Option<SystemTime>,
    pub install: Option<Install>,
    /// 本次安装的取消标记；每次开始安装时换一个新的。
    cancel: Arc<AtomicBool>,
}

impl UpdateState {
    pub fn available(&self) -> Option<&Release> {
        match &self.status {
            Status::Available(release) => Some(release),
            _ => None,
        }
    }
}

/// 启动自动检查的后台线程。是否检查由 `Shared::auto_check_updates` 控制。
pub fn spawn(shared: Arc<Shared>) {
    let spawned = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            std::thread::sleep(STARTUP_DELAY);
            loop {
                let due = shared.update_state().checked_at.is_none_or(|at| {
                    at.elapsed()
                        .map_or(true, |elapsed| elapsed >= CHECK_INTERVAL)
                });
                if due && shared.auto_check_updates.load(Ordering::Relaxed) {
                    check(&shared, false);
                }
                std::thread::sleep(WAKE_INTERVAL);
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("无法启动检查更新线程：{err}");
    }
}

/// 立即检查一次（设置页的“检查更新”），结果通过 `AppEvent::UpdateChanged` 通知 UI。
pub fn check_now(shared: &Arc<Shared>) {
    let shared = shared.clone();
    std::thread::spawn(move || check(&shared, true));
}

fn check(shared: &Shared, manual: bool) {
    let started = shared.modify_update(|state| !std::mem::replace(&mut state.checking, true));
    if !started {
        return;
    }
    let result = fetch_latest();
    shared.modify_update(|state| {
        state.checking = false;
        match result {
            Ok(latest) => {
                state.checked_at = Some(SystemTime::now());
                state.status = match latest {
                    Some(release) if is_newer(&release.version, CURRENT_VERSION) => {
                        tracing::info!("发现新版本 {}", release.version);
                        Status::Available(release)
                    }
                    _ => Status::UpToDate,
                };
            }
            Err(err) => {
                tracing::warn!("检查更新失败：{err:#}");
                // 自动检查失败（例如还没联网）不打扰用户，保留上次的结果，下次醒来再试。
                if manual {
                    state.status = Status::Failed(format!("{err:#}"));
                }
            }
        }
    });
}

/// 当前运行的程序能否在应用内替换；不能时给出原因（例如用 `cargo run` 启动的）。
pub fn install_target() -> Result<&'static Path, &'static str> {
    static TARGET: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    TARGET
        .get_or_init(updater::install_target)
        .as_ref()
        .map(PathBuf::as_path)
        .map_err(String::as_str)
}

/// 更新后是否要重新授予系统权限（macOS 上用临时签名打包时）。只检查一次。
pub fn update_resets_permission() -> bool {
    static RESETS: OnceLock<bool> = OnceLock::new();
    *RESETS.get_or_init(updater::permission_lost_on_update)
}

/// 能否在应用内安装 `release`；不能时只能在浏览器中下载。
pub fn can_install(release: &Release) -> bool {
    release.asset.is_some() && install_target().is_ok()
}

/// 开始下载并安装检查到的新版本；完成后 UI 线程调用 [`restart_if_installed`] 重启。
pub fn install(shared: &Arc<Shared>) {
    let started = shared.modify_update(|state| {
        let release = state.available()?.clone();
        if state.install.as_ref().is_some_and(Install::in_progress) {
            return None;
        }
        let total = release.asset.as_ref().map_or(0, |asset| asset.size);
        state.install = Some(Install::Downloading { received: 0, total });
        state.cancel = Arc::new(AtomicBool::new(false));
        Some((release, state.cancel.clone()))
    });
    let Some((release, cancel)) = started else {
        return;
    };
    let shared = shared.clone();
    let spawned = std::thread::Builder::new()
        .name("update-install".into())
        .spawn(move || run_install(&shared, &release, &cancel));
    if let Err(err) = spawned {
        tracing::warn!("无法启动安装更新线程：{err}");
    }
}

/// 取消下载。界面立即复原；后台线程在下一次读到数据（或超时）时发现已取消，丢弃下载的内容。
pub fn cancel_install(shared: &Shared) {
    shared.modify_update(|state| {
        if matches!(state.install, Some(Install::Downloading { .. })) {
            state.cancel.store(true, Ordering::Relaxed);
            state.install = None;
        }
    });
}

/// 新版本安装好以后由 UI 线程调用：启动新版本并退出当前进程。
/// `args` 是传给新版本的命令行参数（数据目录等），保证重启前后用的是同一份数据。
pub fn restart_if_installed(shared: &Shared, args: &[OsString]) {
    let Some(Install::Installed(target)) = shared.update_state().install else {
        return;
    };
    match updater::relaunch(&target, args) {
        Ok(()) => {
            tracing::info!("已安装新版本，重新启动");
            std::process::exit(0);
        }
        Err(err) => shared.modify_update(|state| {
            state.install = Some(Install::Failed(format!(
                "新版本已经装好，但没能自动启动，请手动打开：{err:#}"
            )))
        }),
    }
}

/// 新版本启动时清理上次更新留下的文件。
pub fn cleanup() {
    updater::cleanup();
    let _ = fs::remove_dir_all(work_root());
}

fn run_install(shared: &Shared, release: &Release, cancel: &AtomicBool) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let work = work_root().join(format!("{}-{nanos}", std::process::id()));
    let result = install_release(shared, release, cancel, &work);
    let _ = fs::remove_dir_all(&work);
    if cancel.load(Ordering::Relaxed) {
        tracing::info!("已取消更新");
        return;
    }
    shared.modify_update(|state| {
        state.install = Some(match result {
            Ok(target) => {
                tracing::info!("已安装 {} 到 {}", release.version, target.display());
                Install::Installed(target)
            }
            Err(err) => {
                tracing::warn!("安装更新失败：{err:#}");
                Install::Failed(format!("{err:#}"))
            }
        });
    });
}

fn install_release(
    shared: &Shared,
    release: &Release,
    cancel: &AtomicBool,
    work: &Path,
) -> Result<PathBuf> {
    let asset = release
        .asset
        .as_ref()
        .context("Release 中没有当前平台的安装包")?;
    let target = install_target().map_err(anyhow::Error::msg)?;
    fs::create_dir_all(work).context("创建临时目录失败")?;
    // 文件名来自 GitHub，只取最后一段。
    let name = Path::new(&asset.name)
        .file_name()
        .context("无效的安装包文件名")?;
    let package = work.join(name);
    download(shared, asset, &package, cancel)?;
    // 与取消在同一把锁里判断，取消之后不会再显示“正在安装”。
    let proceed = shared.modify_update(|state| {
        let proceed = !cancel.load(Ordering::Relaxed);
        if proceed {
            state.install = Some(Install::Installing);
        }
        proceed
    });
    ensure!(proceed, "已取消");
    updater::install(&package, target, &release.version)?;
    Ok(target.to_path_buf())
}

/// 下载安装包到 `path`，核对大小与 SHA-256。
fn download(shared: &Shared, asset: &Asset, path: &Path, cancel: &AtomicBool) -> Result<()> {
    let agent: ureq::Agent = config()
        .timeout_connect(Some(TIMEOUT))
        .timeout_recv_response(Some(TIMEOUT))
        .timeout_recv_body(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into();
    let response = agent
        .get(&asset.url)
        .header("Accept", "application/octet-stream")
        .call()
        .map_err(describe)?;
    let mut reader = response.into_body().into_reader();
    let mut file = fs::File::create(path).context("创建安装包文件失败")?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut received = 0u64;
    let mut reported = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("已取消");
        }
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err).context("下载安装包失败"),
        };
        file.write_all(&buffer[..read]).context("写入安装包失败")?;
        hasher.update(&buffer[..read]);
        received += read as u64;
        ensure!(received <= asset.size, "下载的安装包比 Release 中记录的大");
        if reported.elapsed() >= PROGRESS_INTERVAL {
            reported = Instant::now();
            shared.modify_update(|state| {
                if !cancel.load(Ordering::Relaxed) {
                    state.install = Some(Install::Downloading {
                        received,
                        total: asset.size,
                    });
                }
            });
        }
    }
    file.sync_all().context("写入安装包失败")?;
    ensure!(
        received == asset.size,
        "安装包不完整（{received} / {} 字节）",
        asset.size
    );
    if let Some(expected) = &asset.sha256 {
        let actual = format!("{:x}", hasher.finalize());
        ensure!(actual == *expected, "安装包校验失败（SHA-256 不一致）");
    }
    Ok(())
}

/// 下载安装包用的临时目录。
fn work_root() -> PathBuf {
    std::env::temp_dir().join("DevToolsHub-update")
}

/// 请求 GitHub 的公共配置。
fn config() -> ConfigBuilder<AgentScope> {
    // 用系统自带的 TLS 与根证书（macOS Security.framework、Windows SChannel），
    // 公司网络里装了自签根证书的代理也能正常访问。
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(format!("DevToolsHub/{CURRENT_VERSION}"))
}

/// 查询最新的正式版本；仓库还没有发布过版本时返回 `None`。
fn fetch_latest() -> Result<Option<Release>> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        repo_path(REPOSITORY)
    );
    let agent: ureq::Agent = config().timeout_global(Some(TIMEOUT)).build().into();
    let response = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call();
    let body = match response {
        Ok(mut response) => response
            .body_mut()
            .read_to_string()
            .context("读取 GitHub 的响应失败")?,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(err) => return Err(describe(err)),
    };
    parse_release(&body, &asset_suffix()).map(Some)
}

fn describe(err: ureq::Error) -> anyhow::Error {
    match err {
        // 未登录的接口限制每个 IP 每小时 60 次。
        ureq::Error::StatusCode(403 | 429) => anyhow!("GitHub 接口请求过于频繁，请稍后再试"),
        ureq::Error::StatusCode(code) => anyhow!("GitHub 返回了错误（HTTP {code}）"),
        ureq::Error::Timeout(_) => anyhow!("连接 GitHub 超时"),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => {
            anyhow!("无法连接 GitHub，请检查网络")
        }
        other => anyhow::Error::new(other).context("请求 GitHub 失败"),
    }
}

/// `https://github.com/owner/repo` → `owner/repo`
fn repo_path(repository: &str) -> &str {
    repository
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/')
        .trim_end_matches(".git")
}

/// 当前平台安装包文件名的结尾，与发布流程中的命名一致：
/// `DevTools-Hub-<版本>-macos-arm64.dmg`、`DevTools-Hub-<版本>-windows-x64.exe`。
fn asset_suffix() -> String {
    let (os, extension) = if cfg!(target_os = "macos") {
        ("macos", "dmg")
    } else {
        ("windows", "exe")
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("-{os}-{arch}.{extension}")
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    /// 形如 `sha256:<十六进制>`。
    #[serde(default)]
    digest: Option<String>,
}

fn parse_release(json: &str, asset_suffix: &str) -> Result<Release> {
    let release: ApiRelease = serde_json::from_str(json).context("无法解析 GitHub 的响应")?;
    let version = release
        .tag_name
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    if parse_version(&version).is_none() {
        bail!("无法识别的版本号：{}", release.tag_name);
    }
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name.ends_with(asset_suffix))
        .map(|asset| Asset {
            sha256: asset
                .digest
                .as_deref()
                .and_then(|digest| digest.strip_prefix("sha256:"))
                .map(str::to_ascii_lowercase),
            name: asset.name,
            url: asset.browser_download_url,
            size: asset.size,
        });
    Ok(Release {
        version,
        page_url: release.html_url,
        asset,
    })
}

/// `0.1.10` → `[0, 1, 10]`；可以带前缀 `v`，忽略 `-`、`+` 之后的预发布与构建信息。
fn parse_version(version: &str) -> Option<[u64; 3]> {
    let core = version.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next()?;
    let mut parts = [0; 3];
    for (index, part) in core.split('.').enumerate() {
        *parts.get_mut(index)? = part.parse().ok()?;
    }
    Some(parts)
}

/// `candidate` 是否比 `current` 新。无法识别的版本号一律不算新。
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!(
        (parse_version(candidate), parse_version(current)),
        (Some(candidate), Some(current)) if candidate > current
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.1.2", "0.1.1"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.1", "0.1.1"));
        assert!(!is_newer("v0.1.1", "0.1.1"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("0.1.1-beta.1", "0.1.1"));
        assert!(!is_newer("latest", "0.1.1"));
        assert!(!is_newer("1.2.3.4", "0.1.1"));
        assert!(!is_newer("", "0.1.1"));
    }

    #[test]
    fn parses_release_and_picks_platform_asset() {
        let json = r#"{
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/zvae/devtools_hub/releases/tag/v0.2.0",
            "draft": false,
            "assets": [
                {
                    "name": "DevTools-Hub-0.2.0-windows-x64.exe",
                    "browser_download_url": "https://example.com/win.exe",
                    "size": 2048,
                    "digest": null
                },
                {
                    "name": "DevTools-Hub-0.2.0-macos-arm64.dmg",
                    "browser_download_url": "https://example.com/mac.dmg",
                    "size": 1024,
                    "digest": "sha256:ABCDEF0123"
                }
            ]
        }"#;
        let release = parse_release(json, "-macos-arm64.dmg").unwrap();
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.download_or_page(), "https://example.com/mac.dmg");
        let asset = release.asset.unwrap();
        assert_eq!(asset.size, 1024);
        assert_eq!(asset.sha256.as_deref(), Some("abcdef0123"));
        let release = parse_release(json, "-windows-x64.exe").unwrap();
        let asset = release.asset.unwrap();
        assert_eq!(asset.url, "https://example.com/win.exe");
        assert_eq!(asset.sha256, None);
        // 没有当前平台的安装包时打开 Release 页面。
        let release = parse_release(json, "-macos-x64.dmg").unwrap();
        assert_eq!(release.download_or_page(), release.page_url);

        let bad = r#"{"tag_name": "nightly", "html_url": "https://example.com"}"#;
        assert!(parse_release(bad, "-macos-arm64.dmg").is_err());
    }

    #[test]
    fn derives_repo_path() {
        assert_eq!(
            repo_path("https://github.com/zvae/devtools_hub"),
            "zvae/devtools_hub"
        );
        assert_eq!(
            repo_path("https://github.com/zvae/devtools_hub/"),
            "zvae/devtools_hub"
        );
        // 仓库地址必须是 GitHub 的，否则接口地址拼不对。
        assert!(REPOSITORY.starts_with("https://github.com/"));
        assert_eq!(repo_path(REPOSITORY).split('/').count(), 2);
    }

    #[test]
    fn install_and_cancel_follow_state() {
        let shared = Shared::new(&Settings::default());
        // 没有检查到新版本时不会开始安装。
        install(&shared);
        assert_eq!(shared.update_state().install, None);

        // 正在下载时再点一次不会重新开始；取消后界面立即复原。
        shared.modify_update(|state| {
            state.status = Status::Available(Release {
                version: "9.0.0".into(),
                page_url: "https://example.com".into(),
                asset: None,
            });
            state.install = Some(Install::Downloading {
                received: 10,
                total: 100,
            });
        });
        install(&shared);
        assert_eq!(
            shared.update_state().install,
            Some(Install::Downloading {
                received: 10,
                total: 100
            })
        );
        cancel_install(&shared);
        assert_eq!(shared.update_state().install, None);
        assert!(shared.update_state().cancel.load(Ordering::Relaxed));
    }

    /// 访问真实的 GitHub 接口，默认跳过：
    /// `cargo test -p devtools-hub fetches_latest_release -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn fetches_latest_release() {
        let latest = fetch_latest().unwrap();
        println!("当前 {CURRENT_VERSION}，最新 {latest:?}");
        if let Some(release) = latest {
            assert!(parse_version(&release.version).is_some());
            assert!(release.page_url.starts_with(REPOSITORY));
        }
    }

    /// 下载最新 Release 中当前平台的安装包并核对大小与 SHA-256，默认跳过：
    /// `cargo test -p devtools-hub downloads_release_asset -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn downloads_release_asset() {
        let asset = fetch_latest().unwrap().and_then(|release| release.asset);
        let Some(asset) = asset else {
            println!("最新 Release 中没有当前平台的安装包");
            return;
        };
        println!("下载 {asset:?}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(&asset.name);
        let shared = Shared::new(&Settings::default());
        download(&shared, &asset, &path, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), asset.size);

        // 校验值不对时报错。
        let wrong = Asset {
            sha256: Some("0".repeat(64)),
            ..asset
        };
        let error = download(&shared, &wrong, &path, &AtomicBool::new(false)).unwrap_err();
        assert!(format!("{error:#}").contains("SHA-256"));
    }
}
