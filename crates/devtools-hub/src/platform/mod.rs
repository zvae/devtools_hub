//! 平台能力：剪贴板、来源应用、模拟粘贴、窗口控制、应用图标、开机启动、安装更新、截屏与贴图窗口。
//!
//! macOS 与 Windows 各有一套实现，对外提供相同的模块和函数：
//! `pasteboard`、`workspace`、`keyboard`、`panel`、`icons`、`login_item`、`updater`、
//! `screen`（截屏、显示器与窗口位置）、`floating`（截屏遮罩与贴图窗口）。
//! Windows 另有自绘的右键菜单 `menu`，排布与绘制在与平台无关的 `drawn_menu` 里。

pub mod drawn_menu;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

// 在非 Windows 平台上只做类型检查（不链接、不运行）：
// cargo check -p devtools-hub --features windows-typecheck
#[cfg(all(not(windows), feature = "windows-typecheck"))]
#[allow(dead_code, unused_imports)]
mod windows;

use serde::{Deserialize, Serialize};

/// 文件管理器的名称（右键菜单“在访达中显示”）。
pub const FILE_MANAGER: &str = if cfg!(target_os = "macos") {
    "访达"
} else {
    "资源管理器"
};

/// 状态栏 / 任务栏通知区域图标的称呼。
pub const TRAY_NAME: &str = if cfg!(target_os = "macos") {
    "菜单栏图标"
} else {
    "托盘图标"
};

/// 自动粘贴是否需要用户授予系统权限（macOS 的“辅助功能”；Windows 不需要）。
pub const PASTE_NEEDS_PERMISSION: bool = cfg!(target_os = "macos");

/// 截屏是否需要用户授予系统权限（macOS 的“屏幕录制”；Windows 不需要）。
pub const SCREEN_CAPTURE_NEEDS_PERMISSION: bool = cfg!(target_os = "macos");

/// 是否支持滚动截图（长截图）。Windows 上的截屏遮罩是不透明窗口（wgpu 的透明窗口不稳定），
/// 看不到下面滚动的内容，暂不支持。
pub const SCROLL_CAPTURE_SUPPORTED: bool = cfg!(target_os = "macos");

/// 是否支持通用剪贴板（从 iPhone / iPad 同步内容；Windows 没有）。
pub const HAS_UNIVERSAL_CLIPBOARD: bool = cfg!(target_os = "macos");

/// 面板出现的位置（对应 PasteNow 的 “Appear near Cursor / Appear at Last Position”）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    #[default]
    NearCursor,
    ScreenCenter,
    LastPosition,
}

impl Placement {
    /// 设置里可选的全部位置。
    pub const ALL: [Placement; 3] = [
        Placement::NearCursor,
        Placement::ScreenCenter,
        Placement::LastPosition,
    ];

    /// 界面上显示的名字。
    pub fn label(self) -> &'static str {
        match self {
            Placement::NearCursor => "鼠标附近",
            Placement::ScreenCenter => "屏幕中央",
            Placement::LastPosition => "上次的位置",
        }
    }
}

/// 面板窗口的背景材质，UI 据此决定底色的不透明度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backdrop {
    /// 不透明底色（Windows、离屏渲染测试）。
    #[default]
    Opaque,
    /// macOS 15 及更早：`NSVisualEffectView` 毛玻璃。
    Vibrancy,
    /// macOS 26 起：`NSGlassEffectView` 液态玻璃。
    Glass,
}

/// 托盘图标在屏幕上的位置（物理像素，左上角为原点），来自托盘点击事件。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrayRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 面板从哪里弹出。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// 按设置中的位置（快捷键唤起）。
    Placement(Placement),
    /// 托盘 / 菜单栏图标旁边（点击图标）。macOS 直接读取状态栏窗口的位置，可以不带区域。
    StatusItem(Option<TrayRect>),
}
