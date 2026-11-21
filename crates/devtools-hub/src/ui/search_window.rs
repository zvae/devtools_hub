//! 搜索页的独立窗口（egui 子视口）。
//!
//! 搜索页不再和剪贴板面板共用无边框置顶窗口：这里用 `show_viewport_deferred`
//! 创建一个系统窗口，普通模式下带标题栏、出现在任务栏 / Dock、不置顶；
//! 浮动 / 固定模式保持原来的无边框置顶外观，由模块的窗口策略决定失焦行为。

use devtools_search::SearchWindowMode;
use egui::{Pos2, Rect, Vec2, ViewportBuilder, ViewportId};

/// 搜索窗口的最小内容区尺寸（与设计文档一致）。
pub const MIN_SIZE: Vec2 = Vec2::new(800.0, 480.0);

/// 搜索窗口没有记住尺寸时使用的默认大小。
const DEFAULT_SIZE: Vec2 = Vec2::new(1100.0, 680.0);

/// 搜索窗口的视口 id。
pub fn viewport() -> ViewportId {
    ViewportId::from_hash_of("devtools-search-window")
}

/// 搜索窗口的构建参数。
///
/// 每帧传入同一个 `builder` 时 eframe 只比较构建参数本身，用户拖动 / 缩放窗口不会被覆盖；
/// 模式切换导致标题栏、置顶或任务栏属性变化时才会重新应用。位置在打开时算好，
/// 会话内保持不变（模式切换重建窗口也不会跳回屏幕中央）。
pub fn builder(mode: SearchWindowMode, size: [f32; 2], position: Pos2) -> ViewportBuilder {
    let size = Vec2::new(size[0], size[1]);
    let size = if size.x >= MIN_SIZE.x && size.y >= MIN_SIZE.y {
        size
    } else {
        DEFAULT_SIZE
    };
    let builder = ViewportBuilder::default()
        .with_title("文件搜索")
        .with_inner_size(size)
        .with_min_inner_size(MIN_SIZE)
        .with_resizable(true)
        .with_transparent(false)
        .with_active(true)
        .with_decorations(mode.decorated())
        .with_window_level(if mode.always_on_top() {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        })
        // macOS 上无边框模式没有系统阴影，普通窗口跟随系统。
        .with_has_shadow(true)
        .with_position(position);
    // Windows 的任务栏按钮只能在建窗时决定（变化会重建窗口）；macOS 的 Dock 图标由激活策略控制。
    #[cfg(windows)]
    let builder = builder.with_taskbar(mode.in_taskbar());
    builder
}

/// 让窗口出现在鼠标所在屏幕的中央（搜索页像普通文档窗口一样打开）。
pub fn centered_position(size: [f32; 2], monitor: Option<Rect>) -> Pos2 {
    let size = Vec2::new(size[0], size[1]);
    let monitor = monitor.unwrap_or_else(|| {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))
    });
    Pos2::new(
        (monitor.min.x + (monitor.width() - size.x) * 0.5).max(monitor.min.x),
        (monitor.min.y + (monitor.height() - size.y) * 0.5).max(monitor.min.y),
    )
}
