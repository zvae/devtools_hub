//! DevTools Hub：类 PasteNow 的剪贴板历史管理器，支持 macOS 与 Windows。
//!
//! 结构：
//! - `monitor`：后台线程轮询剪贴板序号（macOS 的 changeCount / Windows 的
//!   GetClipboardSequenceNumber），按规则记录到 SQLite；
//! - `hotkey` / `tray`：全局快捷键与菜单栏 / 托盘图标，唤起面板；
//! - `middle_click` / `selection` / `tools`：鼠标中键快捷面板（参照 uTools）：读取其他应用中
//!   选中的内容，列出能处理它的功能（目前是 JSON 编辑器）；
//! - `snip`：截屏与贴图（参照 Snipaste）中与平台、界面无关的计算，界面在 `ui::snipper` / `ui::pin`；
//! - `ui`：egui 面板（搜索、分类、预览、键盘操作）与设置页；
//! - `update`：检查 GitHub Releases 上的新版本，提示升级并在应用内安装；
//! - `platform`：各平台的原生能力（剪贴板读写、模拟粘贴 / 复制、鼠标中键监听、窗口控制、
//!   程序图标、开机启动）。

// Windows 发布版不弹出控制台窗口；调试版保留控制台以便查看日志。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(any(target_os = "macos", windows))]
mod controller;
#[cfg(any(target_os = "macos", windows))]
mod hotkey;
mod keys;
#[cfg(any(target_os = "macos", windows))]
mod middle_click;
#[cfg(any(target_os = "macos", windows))]
mod monitor;
#[cfg(any(target_os = "macos", windows))]
mod platform;
#[cfg(any(target_os = "macos", windows))]
mod selection;
#[cfg(any(target_os = "macos", windows))]
mod settings;
#[cfg(any(target_os = "macos", windows))]
mod shared;
#[cfg(any(target_os = "macos", windows))]
mod snip;
#[cfg(any(target_os = "macos", windows))]
mod tools;
#[cfg(any(target_os = "macos", windows))]
mod tray;
#[cfg(any(target_os = "macos", windows))]
mod ui;
#[cfg(any(target_os = "macos", windows))]
mod update;

use std::path::PathBuf;

/// 命令行参数。
#[derive(Clone, Debug, Default)]
pub struct Args {
    /// 启动后立即显示面板。
    pub show: bool,
    pub data_dir: Option<PathBuf>,
    /// 监听指定名称的私有剪贴板而不是系统剪贴板（调试用）。
    pub pasteboard: Option<String>,
    /// 由旧版本在应用内更新后启动：等旧进程退出，启动后显示设置页底部的版本信息。
    pub after_update: bool,
    /// 启动后立即截屏（对应 Snipaste 的命令行 `snip`）。
    pub snip: bool,
    /// 启动后把这张图片贴到屏幕上（对应 Snipaste 的命令行 `paste --files`）。
    pub pin: Option<PathBuf>,
}

/// `--help` 的输出。
const HELP: &str = "DevTools Hub 剪贴板历史

用法: devtools-hub [选项]

选项:
  --show                 启动后立即显示面板
  --data-dir <目录>      数据目录（默认 macOS 为 ~/Library/Application Support/DevToolsHub，
                         Windows 为 %LOCALAPPDATA%\\DevToolsHub；
                         也可用环境变量 DEVTOOLS_HUB_DATA_DIR 指定）
  --pasteboard <名称>    监听指定名称的私有剪贴板（调试用，仅 macOS，不影响系统剪贴板）
  --snip                 启动后立即截屏
  --pin <图片>           启动后把图片贴到屏幕上
  -h, --help             显示帮助";

impl Args {
    /// 解析命令行参数；`--help` 时打印帮助并返回 `None`。
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
                "--snip" => args.snip = true,
                "--pin" => args.pin = Some(iter.next().ok_or("--pin 需要一张图片")?.into()),
                // 内部参数，不在帮助中列出。
                "--after-update" => args.after_update = true,
                "-h" | "--help" => return Ok(None),
                // Finder/launchd 启动 .app 时可能附带 -psn_ 参数
                other if other.starts_with("-psn_") => {}
                other => return Err(format!("未知参数：{other}")),
            }
        }
        Ok(Some(args))
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn main() {
    eprintln!("DevTools Hub 剪贴板历史目前只支持 macOS 和 Windows。");
    std::process::exit(1);
}

#[cfg(any(target_os = "macos", windows))]
fn main() -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

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
    // 应用内更新后由旧版本启动时，旧进程可能还没退出，稍等它释放锁。
    let lock_file = std::fs::File::create(data_dir.join(".lock")).context("创建锁文件失败")?;
    let wait = if args.after_update {
        Duration::from_secs(15)
    } else {
        Duration::ZERO
    };
    let deadline = Instant::now() + wait;
    while lock_file.try_lock().is_err() {
        if Instant::now() >= deadline {
            eprintln!(
                "DevTools Hub 已经在运行（数据目录 {}）。",
                data_dir.display()
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    update::cleanup();

    controller::init_main_thread();
    let settings = settings::Settings::load(&data_dir);
    let shared = shared::Shared::new(&settings);
    let monitor_store =
        devtools_clipboard::Store::open(&data_dir).context("打开剪贴板数据库失败")?;
    let _monitor = monitor::spawn(shared.clone(), monitor_store, args.pasteboard.clone());
    update::spawn(shared.clone());

    let viewport = egui::ViewportBuilder::default()
        .with_title("DevTools Hub")
        .with_inner_size(settings.window_size)
        // 同一个窗口也用作鼠标中键快捷面板，最小尺寸要容得下它。
        .with_min_inner_size([240.0, 120.0])
        .with_decorations(false)
        .with_resizable(true)
        .with_visible(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop);
    // macOS：透明窗口 + 原生毛玻璃背景。应用图标用 .app 里的 AppIcon.icns，
    // 传空图标是为了阻止 eframe 在运行时把它换成 egui 的默认图标。
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_transparent(true)
        .with_has_shadow(true)
        .with_icon(egui::IconData::default());
    // Windows：不在任务栏显示按钮（只在通知区域常驻）；不透明窗口，圆角交给 DWM。
    // 窗口图标在 Alt+Tab 中显示；exe 文件的图标由 build.rs 嵌入。
    #[cfg(windows)]
    let viewport = {
        let icon = image::load_from_memory(include_bytes!("../assets/icon.png"))
            .context("解码窗口图标失败")?
            .into_rgba8();
        viewport.with_taskbar(false).with_icon(egui::IconData {
            width: icon.width(),
            height: icon.height(),
            rgba: icon.into_raw(),
        })
    };

    let options = eframe::NativeOptions {
        viewport,
        centered: true,
        event_loop_builder: event_loop_hook(),
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

/// 等价于 Info.plist 中的 LSUIElement：不显示 Dock 图标，只在菜单栏常驻。
#[cfg(target_os = "macos")]
fn event_loop_hook() -> Option<eframe::EventLoopBuilderHook> {
    Some(Box::new(|builder| {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_activate_ignoring_other_apps(false);
    }))
}

#[cfg(windows)]
fn event_loop_hook() -> Option<eframe::EventLoopBuilderHook> {
    None
}
