//! 浮动面板窗口控制。
//!
//! PasteNow 用自定义的 `NSPanel` 子类（`FloatPanel` / `PasteFloatPanel`）实现，
//! 背景是 `NSVisualEffectView` 毛玻璃；从菜单栏点开时贴在状态栏图标下方
//! （日志 “The status item screen is: …, visible frame is: …”），用快捷键唤起时
//! 根据 `NSEvent.mouseLocation` 找到鼠标所在的 `NSScreen` 定位。
//! eframe 创建的是普通 `NSWindow`，这里直接通过原生 API 控制它：
//! 显示时激活本应用并记住之前的前台应用，隐藏时把焦点还给它，再由调用方发送 ⌘V。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use super::workspace;
pub use crate::platform::{Anchor, Placement};
use anyhow::{bail, Context, Result};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSAutoresizingMaskOptions, NSEvent,
    NSRunningApplication, NSScreen, NSStatusWindowLevel, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
    NSWindowCollectionBehavior, NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

thread_local! {
    static PANEL: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
    static PREVIOUS_APP: RefCell<Option<Retained<NSRunningApplication>>> = const { RefCell::new(None) };
}

/// 窗口圆角半径，与 UI 中背景的圆角一致。
pub const CORNER_RADIUS: f64 = 12.0;

static VIBRANCY: AtomicBool = AtomicBool::new(false);

/// 绑定 eframe 创建的窗口，并设置成浮动面板的行为。必须在主线程调用。
///
/// `start_hidden`：eframe 会在第一帧之后强制显示窗口，这里先把窗口设为全透明，
/// 之后由 [`finish_startup_hide`] 收起，避免启动时闪出一个面板。
pub fn attach(handle: &impl HasWindowHandle, start_hidden: bool) -> Result<()> {
    MainThreadMarker::new().context("必须在主线程绑定窗口")?;
    let RawWindowHandle::AppKit(raw) = handle.window_handle()?.as_raw() else {
        bail!("不是 AppKit 窗口");
    };
    // SAFETY: eframe 保证句柄在窗口存活期间有效，NSView 指针来自 winit。
    let view: &NSView = unsafe { raw.ns_view.cast().as_ref() };
    let window = view.window().context("视图不在窗口中")?;
    // 出现在所有桌面（含全屏应用所在的桌面）之上，像系统面板一样。
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.setLevel(NSStatusWindowLevel);
    window.setHidesOnDeactivate(false);
    if start_hidden {
        window.setAlphaValue(0.0);
    }
    let mtm = MainThreadMarker::from(&*window);
    let vibrancy = install_vibrancy(mtm, &window);
    VIBRANCY.store(vibrancy, Ordering::Relaxed);
    tracing::debug!(
        "已绑定面板窗口 #{}，毛玻璃背景：{vibrancy}",
        window.windowNumber()
    );
    PANEL.with(|panel| *panel.borrow_mut() = Some(window));
    Ok(())
}

/// 在内容视图下方插入一个 `NSVisualEffectView` 作为毛玻璃背景。
///
/// 不能替换 contentView（winit 依赖它），所以放在窗口框架视图里、位于内容视图之下；
/// 窗口本身是透明的，egui 只画半透明的底色，背后的模糊效果就能透出来。
fn install_vibrancy(mtm: MainThreadMarker, window: &NSWindow) -> bool {
    let Some(content) = window.contentView() else {
        return false;
    };
    // SAFETY: 只读取父视图引用。
    let Some(frame_view) = (unsafe { content.superview() }) else {
        return false;
    };
    let effect = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), content.frame());
    effect.setMaterial(NSVisualEffectMaterial::Popover);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    effect.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    effect.setWantsLayer(true);
    // SAFETY: setWantsLayer(true) 之后 layer 是 CALayer，这两个方法都存在。
    unsafe {
        let layer: Option<Retained<AnyObject>> = msg_send![&*effect, layer];
        if let Some(layer) = layer {
            let _: () = msg_send![&*layer, setCornerRadius: CORNER_RADIUS];
            let _: () = msg_send![&*layer, setMasksToBounds: true];
        }
    }
    frame_view.addSubview_positioned_relativeTo(
        &effect,
        NSWindowOrderingMode::Below,
        Some(&content),
    );
    window.invalidateShadow();
    true
}

/// 收起启动时被 eframe 显示出来的（透明）窗口，并恢复不透明度。
pub fn finish_startup_hide() {
    with_panel(|_, window| {
        window.orderOut(None);
        window.setAlphaValue(1.0);
    });
}

/// 是否成功启用了毛玻璃背景（UI 据此决定底色是否半透明）。
pub fn has_vibrancy() -> bool {
    VIBRANCY.load(Ordering::Relaxed)
}

fn with_panel<R>(f: impl FnOnce(MainThreadMarker, &NSWindow) -> R) -> Option<R> {
    let mtm = MainThreadMarker::new()?;
    PANEL.with(|panel| panel.borrow().as_ref().map(|window| f(mtm, window)))
}

/// 面板是否正在显示且本应用处于前台。
pub fn is_active() -> bool {
    with_panel(|mtm, window| window.isVisible() && NSApplication::sharedApplication(mtm).isActive())
        .unwrap_or(false)
}

pub fn is_visible() -> bool {
    with_panel(|_, window| window.isVisible()).unwrap_or(false)
}

pub fn show(anchor: Anchor) -> bool {
    with_panel(|mtm, window| {
        if !window.isVisible() {
            if let Some(app) = workspace::frontmost_other_app() {
                PREVIOUS_APP.with(|prev| *prev.borrow_mut() = Some(app));
            }
            match anchor {
                Anchor::Placement(placement) => position(mtm, window, placement),
                Anchor::StatusItem(_) => position_below_status_item(mtm, window),
            }
        }
        window.setAlphaValue(1.0);
        let app = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        window.makeKeyAndOrderFront(None);
        window.orderFrontRegardless();
        let frame = window.frame();
        tracing::debug!(
            "显示面板：{anchor:?} 原点 ({:.0}, {:.0}) 尺寸 {:.0}×{:.0}，之前的前台应用 {:?}",
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
            previous_app_name()
        );
    })
    .is_some()
}

/// 隐藏面板；`restore_focus` 为 true 时激活弹出面板之前的前台应用。
pub fn hide(restore_focus: bool) {
    with_panel(|mtm, window| {
        window.orderOut(None);
        let previous = PREVIOUS_APP.with(|prev| prev.borrow_mut().take());
        tracing::debug!("隐藏面板，恢复焦点：{restore_focus}");
        if !restore_focus {
            return;
        }
        let app = NSApplication::sharedApplication(mtm);
        match previous {
            Some(previous) if !previous.isTerminated() => {
                previous.activateWithOptions(NSApplicationActivationOptions::empty());
            }
            // 找不到之前的应用时，隐藏自身让系统激活下一个应用。
            _ => app.hide(None),
        }
    });
}

/// 之前的前台应用名称（面板标题里显示“粘贴到 xxx”）。
pub fn previous_app_name() -> Option<String> {
    PREVIOUS_APP.with(|prev| {
        prev.borrow()
            .as_ref()
            .and_then(|app| app.localizedName())
            .map(|name| name.to_string())
    })
}

fn position(mtm: MainThreadMarker, window: &NSWindow, placement: Placement) {
    let frame = window.frame();
    let mouse = NSEvent::mouseLocation();
    let screens = NSScreen::screens(mtm);
    let screen_frames: Vec<NSRect> = screens.iter().map(|s| s.visibleFrame()).collect();
    let mouse_screen = screen_frames
        .iter()
        .copied()
        .find(|vf| contains(*vf, mouse))
        .or_else(|| screen_frames.first().copied());
    let Some(vf) = mouse_screen else {
        return;
    };

    let (x, top) = match placement {
        Placement::NearCursor => (mouse.x + 6.0, mouse.y - 6.0),
        Placement::ScreenCenter => (
            vf.origin.x + (vf.size.width - frame.size.width) / 2.0,
            vf.origin.y + (vf.size.height + frame.size.height) / 2.0 + vf.size.height * 0.08,
        ),
        Placement::LastPosition => {
            let visible_somewhere = screen_frames.iter().any(|vf| intersects(*vf, frame));
            if visible_somewhere {
                return;
            }
            return position(mtm, window, Placement::ScreenCenter);
        }
    };
    let x = x.clamp(
        vf.origin.x,
        (vf.origin.x + vf.size.width - frame.size.width).max(vf.origin.x),
    );
    let min_top = (vf.origin.y + frame.size.height).min(vf.origin.y + vf.size.height);
    let top = top.clamp(min_top, vf.origin.y + vf.size.height);
    window.setFrameTopLeftPoint(NSPoint::new(x, top));
}

/// 贴在菜单栏图标正下方，左边缘与图标大致对齐（与 PasteNow 一致）。
fn position_below_status_item(mtm: MainThreadMarker, window: &NSWindow) {
    let app = NSApplication::sharedApplication(mtm);
    let status_frame = app
        .windows()
        .iter()
        .filter(|w| {
            w.class()
                .name()
                .to_bytes()
                .starts_with(b"NSStatusBarWindow")
        })
        .map(|w| w.frame())
        // 刚创建、尚未布局的状态栏窗口高度为 0，不能用来定位。
        .find(|frame| frame.size.height > 0.0 && frame.size.width > 0.0);
    // 找不到状态栏窗口时退回到鼠标位置（点击菜单栏图标时鼠标就在图标上）。
    let anchor = status_frame.unwrap_or_else(|| {
        let mouse = NSEvent::mouseLocation();
        NSRect::new(
            NSPoint::new(mouse.x - 12.0, mouse.y - 12.0),
            objc2_foundation::NSSize::new(24.0, 24.0),
        )
    });
    tracing::debug!(
        "状态栏图标位置：{:?}（{}）",
        (
            anchor.origin.x,
            anchor.origin.y,
            anchor.size.width,
            anchor.size.height
        ),
        if status_frame.is_some() {
            "NSStatusBarWindow"
        } else {
            "鼠标位置"
        }
    );
    let center = NSPoint::new(anchor.origin.x + anchor.size.width / 2.0, anchor.origin.y);
    let screens: Vec<NSRect> = NSScreen::screens(mtm)
        .iter()
        .map(|s| s.visibleFrame())
        .collect();
    let Some(vf) = screens
        .iter()
        .copied()
        .find(|vf| center.x >= vf.origin.x && center.x <= vf.origin.x + vf.size.width)
        .or_else(|| screens.first().copied())
    else {
        return;
    };
    let frame = window.frame();
    let x = (anchor.origin.x - 10.0).clamp(
        vf.origin.x + 4.0,
        (vf.origin.x + vf.size.width - frame.size.width - 4.0).max(vf.origin.x),
    );
    let top = (anchor.origin.y - 4.0)
        .min(vf.origin.y + vf.size.height - 4.0)
        .max(vf.origin.y + frame.size.height);
    window.setFrameTopLeftPoint(NSPoint::new(x, top));
}

fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x <= rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y <= rect.origin.y + rect.size.height
}

fn intersects(a: NSRect, b: NSRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}
