//! 平台能力：剪贴板、来源应用、模拟粘贴、窗口控制、应用图标、开机启动、安装更新、截屏与贴图窗口。
//!
//! macOS 与 Windows 各有一套实现，对外提供相同的模块和函数：
//! `pasteboard`、`workspace`、`keyboard`、`panel`、`icons`、`login_item`、`updater`、
//! `screen`（截屏、显示器与窗口位置）、`floating`（截屏遮罩与贴图窗口）。
//! Windows 另有自绘的右键菜单 `menu`，排布与绘制在与平台无关的 `drawn_menu` 里。

pub mod drawn_menu;
/// 图片缩略图（与平台无关：`image` 解码，两个平台共用）。
pub mod thumbnails;
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

/// 图标视图里画的东西：优先系统预览，其次按尺寸渲染的系统图标。
///
/// - **预览**：与资源管理器一致（图片、视频首帧、PDF / Office，取决于装了哪些缩略图处理器）；
/// - **图标**：没有预览时由 shell 按目标尺寸出图（不是 `SHGetFileInfoW` 的 32×32 小图标，
///   放大到 96 也不会糊）。
///
/// Windows 走 `IShellItemImageFactory`（先 `THUMBNAILONLY` 再 `ICONONLY`）；macOS 的
/// QuickLook 还没接，先用 `NSWorkspace` 按尺寸取图标。最后才用 `image` 自己解码兜底。
///
/// **调用很慢**（几十毫秒到几百毫秒，取决于格式），必须在后台线程里调用。
pub fn thumbnail(path: &std::path::Path, size: u32) -> Option<Thumbnail> {
    #[cfg(windows)]
    {
        // 目录没有"内容预览"，直接要按尺寸渲染的文件夹图标（省一次 shell 调用）。
        let is_dir = std::fs::metadata(path).map(|meta| meta.is_dir()).unwrap_or(false);
        if !is_dir {
            if let Some((width, height, rgba)) = windows::shell_thumbnail::thumbnail(path, size) {
                return Some(Thumbnail {
                    width,
                    height,
                    rgba,
                    preview: true,
                });
            }
        }
        if let Some((width, height, rgba)) = windows::shell_thumbnail::icon(path, size) {
            return Some(Thumbnail {
                width,
                height,
                rgba,
                // 图标按扩展名就能复用，交给界面缓存。
                preview: false,
            });
        }
    }
    // macOS：没有预览接口时按尺寸取系统图标（`iconForFile` 支持任意尺寸）。
    #[cfg(target_os = "macos")]
    if let Some((width, height, rgba)) = icons::file_icon_rgba(path, size) {
        return Some(Thumbnail {
            width,
            height,
            rgba,
            preview: false,
        });
    }
    // 最后的兜底：图片至少还能自己解码出来。
    thumbnails::image_thumbnail(path, size).map(|(width, height, rgba)| Thumbnail {
        width,
        height,
        rgba,
        preview: true,
    })
}

/// 这个平台是否用系统自带的右键菜单（Windows 是；macOS 继续用 egui 画的菜单）。
pub fn has_system_context_menu() -> bool {
    cfg!(windows)
}

use devtools_search::Thumbnail;

/// 系统右键菜单里用户选了什么（宿主自己处理，模块据此执行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuChoice {
    /// 点了外面 / 按 Esc 取消，或者根本没弹出来。
    Dismissed,
    /// 菜单顶部固定项：用默认程序打开。
    Open,
    /// 菜单顶部固定项：打开所在位置（资源管理器里选中它）。
    Reveal,
    /// 菜单顶部固定项：复制完整路径（含文件名）。
    CopyPath,
    /// shell 自己的命令（"打开方式""属性"、第三方扩展…），已经执行完。
    Handled,
}

/// 在鼠标位置弹出系统右键菜单（资源管理器同款，含第三方扩展）。
///
/// **这是模态调用**，选完（或取消）才返回。
pub fn system_context_menu(path: &std::path::Path) -> MenuChoice {
    #[cfg(windows)]
    {
        windows::shell_menu::show_for_path(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        MenuChoice::Dismissed
    }
}

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

/// 是否支持滚动截图（长截图）。macOS 上遮罩是透明窗口，选区内鼠标穿透；Windows 上遮罩不透明，
/// 把选区从窗口里挖掉（见 `floating::set_scroll_hole`）。
pub const SCROLL_CAPTURE_SUPPORTED: bool = cfg!(any(target_os = "macos", windows));

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
