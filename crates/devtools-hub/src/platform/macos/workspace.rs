//! 前台应用与系统打开操作。

use devtools_clipboard::SourceApp;
use objc2::rc::{autoreleasepool, Retained};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::{NSString, NSURL};

/// 复制发生时的前台应用（PasteNow 同样在检测到变化时读取 `frontmostApplication`）。
/// 面板不激活本应用：在面板里复制时前台仍是别的应用，这时记为本应用。
pub fn frontmost_app() -> SourceApp {
    autoreleasepool(|_| {
        let app = if super::panel::is_key() {
            Some(NSRunningApplication::currentApplication())
        } else {
            NSWorkspace::sharedWorkspace().frontmostApplication()
        };
        app.map(|app| describe(&app)).unwrap_or_default()
    })
}

pub fn describe(app: &NSRunningApplication) -> SourceApp {
    SourceApp {
        bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
        name: app.localizedName().map(|s| s.to_string()),
    }
}

/// 用户是否正在使用本应用（例如正在使用 JSON 编辑器）。可以在任意线程调用。
/// 面板不激活本应用，所以除了本应用在前台，面板是键盘窗口也算。
pub fn is_self_active() -> bool {
    super::panel::is_key()
        || autoreleasepool(|_| NSRunningApplication::currentApplication().isActive())
}

/// 当前前台应用（排除自己），用于粘贴前把焦点还给它。
pub fn frontmost_other_app() -> Option<Retained<NSRunningApplication>> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    (app.processIdentifier() != std::process::id() as i32).then_some(app)
}

pub fn open_url(url: &str) -> bool {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return false;
    };
    NSWorkspace::sharedWorkspace().openURL(&url)
}

pub fn reveal_path(path: &str) {
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
}

pub fn open_path(path: &std::path::Path) {
    let _ = std::process::Command::new("open").arg(path).spawn();
}

/// macOS 不需要持续记录前台应用：面板弹出时前台仍是用户所在的程序
/// （`panel::show` 直接用 `frontmost_other_app`），这里只为共用代码提供空实现。
pub fn remember_foreground() {}
