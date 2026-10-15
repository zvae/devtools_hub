//! DevTools Hub：类 PasteNow 的剪贴板历史管理器。
//!
//! 结构：
//! - `monitor`：后台线程轮询 NSPasteboard 的 changeCount，按规则记录到 SQLite；
//! - `hotkey` / `tray`：全局快捷键与菜单栏图标，唤起面板；
//! - `ui`：egui 面板（搜索、分类、预览、键盘操作）与设置页；
//! - `platform`：macOS 原生能力（剪贴板读写、模拟 ⌘V、窗口控制、开机启动）。

#[cfg(target_os = "macos")]
mod controller;
#[cfg(target_os = "macos")]
mod hotkey;
#[cfg(target_os = "macos")]
mod monitor;
#[cfg(target_os = "macos")]
mod platform;
#[cfg(target_os = "macos")]
mod settings;
#[cfg(target_os = "macos")]
mod shared;
#[cfg(target_os = "macos")]
mod tray;
#[cfg(target_os = "macos")]
mod ui;

use std::path::PathBuf;

/// 命令行参数。
#[derive(Clone, Debug, Default)]
pub struct Args {
    /// 启动后立即显示面板。
    pub show: bool,
    pub data_dir: Option<PathBuf>,
    /// 监听指定名称的私有剪贴板而不是系统剪贴板（调试用）。
    pub pasteboard: Option<String>,
}

const HELP: &str = "DevTools Hub 剪贴板历史

用法: devtools-hub [选项]

选项:
  --show                 启动后立即显示面板
  --data-dir <目录>      数据目录（默认 ~/Library/Application Support/DevToolsHub，
                         也可用环境变量 DEVTOOLS_HUB_DATA_DIR 指定）
  --pasteboard <名称>    监听指定名称的私有剪贴板（调试用，不影响系统剪贴板）
  -h, --help             显示帮助";

impl Args {
    fn parse() -> Result<Option<Self>, String> {
        let mut args = Args::default();
        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--show" => args.show = true,
                "--data-dir" => {
                    args.data_dir = Some(iter.next().ok_or("--data-dir 需要一个目录")?.into())
                }
                "--pasteboard" => {
                    args.pasteboard = Some(iter.next().ok_or("--pasteboard 需要一个名称")?)
                }
                "-h" | "--help" => return Ok(None),
                // Finder/launchd 启动 .app 时可能附带 -psn_ 参数
                other if other.starts_with("-psn_") => {}
                other => return Err(format!("未知参数：{other}")),
            }
        }
        Ok(Some(args))
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("DevTools Hub 剪贴板历史目前只支持 macOS。");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use anyhow::{anyhow, Context};
    use tracing_subscriber::EnvFilter;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,wgpu=warn,naga=warn,eframe=warn")),
        )
        .init();

    let args = match Args::parse() {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{HELP}");
            return Ok(());
        }
        Err(err) => {
            eprintln!("{err}\n\n{HELP}");
            std::process::exit(2);
        }
    };

    let data_dir = settings::resolve_data_dir(args.data_dir.clone())?;
    // 同一数据目录只允许一个实例，否则会重复记录、重复注册快捷键。
    let lock_file = std::fs::File::create(data_dir.join(".lock")).context("创建锁文件失败")?;
    if lock_file.try_lock().is_err() {
        eprintln!(
            "DevTools Hub 已经在运行（数据目录 {}）。",
            data_dir.display()
        );
        return Ok(());
    }

    let settings = settings::Settings::load(&data_dir);
    let shared = shared::Shared::new(&settings);
    let monitor_store =
        devtools_clipboard::Store::open(&data_dir).context("打开剪贴板数据库失败")?;
    let _monitor = monitor::spawn(shared.clone(), monitor_store, args.pasteboard.clone());

    let viewport = egui::ViewportBuilder::default()
        .with_title("DevTools Hub")
        .with_inner_size(settings.window_size)
        .with_min_inner_size([320.0, 400.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_has_shadow(true)
        .with_resizable(true)
        .with_visible(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop);
    let options = eframe::NativeOptions {
        viewport,
        centered: true,
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            // 等价于 Info.plist 中的 LSUIElement：不显示 Dock 图标，只在菜单栏常驻。
            builder
                .with_activation_policy(ActivationPolicy::Accessory)
                .with_activate_ignoring_other_apps(false);
        })),
        ..Default::default()
    };

    eframe::run_native(
        "DevTools Hub",
        options,
        Box::new(move |cc| {
            let app = ui::HubApp::new(cc, shared, data_dir, settings, &args)?;
            Ok(Box::new(app))
        }),
    )
    .map_err(|err| anyhow!("{err}"))?;
    drop(lock_file);
    Ok(())
}
