//! 浮动面板窗口控制。
//!
//! PasteNow 用自定义的 `NSPanel` 子类（`FloatPanel` / `PasteFloatPanel`）实现，
//! 背景是 `NSVisualEffectView` 毛玻璃（这里在 macOS 26 起换成液态玻璃）；从菜单栏点开时贴在状态栏图标下方
//! （日志 “The status item screen is: …, visible frame is: …”），用快捷键唤起时
//! 根据 `NSEvent.mouseLocation` 找到鼠标所在的 `NSScreen` 定位。
//!
//! 与 PasteNow 一致，面板不激活本应用（实测：从菜单栏弹出 PasteNow 面板时前台应用不变，
//! 面板窗口层级是 101，即 `NSPopUpMenuWindowLevel`）。eframe 创建的是普通 `NSWindow`，
//! 绑定时把它换成 `NSPanel` 子类并加上 `NonactivatingPanel` 样式：面板成为键盘窗口，
//! 前台应用保持不变，收起面板后键盘焦点自然回到它，调用方直接发送 ⌘V 即可。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use super::workspace;
pub use crate::platform::{Anchor, Backdrop, Placement};
use anyhow::{bail, Context, Result};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{define_class, msg_send, sel, ClassType, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSAutoresizingMaskOptions, NSEvent,
    NSEventModifierFlags, NSEventType, NSGlassEffectView, NSGlassEffectViewStyle, NSPanel,
    NSPopUpMenuWindowLevel, NSResponder, NSRunningApplication, NSScreen, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowCollectionBehavior, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

thread_local! {
    /// 面板窗口（eframe 创建的窗口，绑定后保存在这里，只在主线程访问）。
    static PANEL: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
    /// 弹出面板前的前台应用，收起面板时把焦点还给它。
    static PREVIOUS_APP: RefCell<Option<Retained<NSRunningApplication>>> = const { RefCell::new(None) };
}

/// 面板是否已换成不激活应用的 `NSPanel`。转换失败时退回到激活本应用的旧做法。
static NONACTIVATING: AtomicBool = AtomicBool::new(false);

/// 面板是否是键盘窗口。面板不激活本应用，后台线程（中键监听、剪贴板监听）
/// 不能再用“本应用是否在前台”判断用户是否正在使用面板。
static PANEL_KEY: AtomicBool = AtomicBool::new(false);

define_class!(
    /// 面板窗口的类：绑定时把 winit 的 `WinitWindow` 实例换成这个类（两者都没有实例变量）。
    ///
    /// SAFETY: `NSPanel` 可以被继承；这里没有实例变量，也不需要 `Drop`。
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DevToolsHubPanel"]
    struct HubPanel;

    impl HubPanel {
        /// 无边框窗口默认不能成为键盘窗口（winit 的 `WinitWindow` 同样覆盖了这个方法）。
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        /// 面板不做主窗口，与系统面板一致。
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }

        /// 记录面板成为 / 不再是键盘窗口（`is_active` 用）。
        #[unsafe(method(becomeKeyWindow))]
        fn become_key_window(&self) {
            PANEL_KEY.store(true, Ordering::Relaxed);
            // SAFETY: 调用父类的同名方法，无参数、无返回值。
            unsafe { msg_send![super(self), becomeKeyWindow] }
        }

        #[unsafe(method(resignKeyWindow))]
        fn resign_key_window(&self) {
            PANEL_KEY.store(false, Ordering::Relaxed);
            // SAFETY: 同上。
            unsafe { msg_send![super(self), resignKeyWindow] }
        }
    }
);

/// 面板是否是键盘窗口（用户正在面板里操作）。可以在任意线程调用。
pub fn is_key() -> bool {
    PANEL_KEY.load(Ordering::Relaxed)
}

/// 把 winit 创建的窗口换成 [`HubPanel`] 并设为不激活应用的面板。返回是否成功。
fn make_nonactivating(window: &NSWindow) -> bool {
    let from = window.class();
    let to = HubPanel::class();
    // 换类要求两个类的实例布局一致：都只有 NSWindow 的实例变量。
    if from.instance_size() != to.instance_size() {
        tracing::warn!(
            "无法把面板换成 NSPanel：{} 实例大小 {}，{} 实例大小 {}",
            from.name().to_string_lossy(),
            from.instance_size(),
            to.name().to_string_lossy(),
            to.instance_size()
        );
        return false;
    }
    // SAFETY: 实例大小相同，WinitWindow 与 HubPanel 都没有自己的实例变量；
    // HubPanel 覆盖的方法保留了 WinitWindow 依赖的行为（可以成为键盘窗口）。
    unsafe { AnyObject::set_class(window, to) };
    window.setStyleMask(window.styleMask() | NSWindowStyleMask::NonactivatingPanel);
    // 只有 init 时带上 NonactivatingPanel，AppKit 才会设置窗口的“阻止激活”属性；
    // 创建后再加这一位，样式里有了，点击面板却仍会激活本应用（实测 `_preventsActivation` 仍为 false）。
    // 没有公开 API，用 NSPanel 初始化时调用的私有方法补上；旧系统上没有这个方法时只能接受点击激活。
    let selector = sel!(_setPreventsActivation:);
    // SAFETY: 先确认方法存在；参数是 BOOL。
    unsafe {
        let responds: bool = msg_send![window, respondsToSelector: selector];
        if responds {
            let _: () = msg_send![window, _setPreventsActivation: true];
        } else {
            tracing::warn!("系统没有 _setPreventsActivation:，点击面板会激活本应用");
        }
    }
    true
}

/// 窗口圆角半径，与 UI 中背景的圆角一致。
///
/// 与 PasteNow 一致：它的面板是标题栏透明的普通窗口（`Titled | FullSizeContentView`），
/// macOS 26 给这类窗口的圆角是 16pt（`NSWindow._cornerRadius`；带工具栏的窗口是 26pt）。
pub const CORNER_RADIUS: f64 = 16.0;

/// 绑定窗口时实际用上的背景材质。
static BACKDROP: OnceLock<Backdrop> = OnceLock::new();

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
    NONACTIVATING.store(make_nonactivating(&window), Ordering::Relaxed);
    // 出现在所有桌面（含全屏应用所在的桌面）之上，像系统面板一样。
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.setLevel(NSPopUpMenuWindowLevel);
    // NSPanel 默认在应用失去激活时隐藏；面板本来就不激活应用，必须关掉。
    window.setHidesOnDeactivate(false);
    if start_hidden {
        window.setAlphaValue(0.0);
    }
    let mtm = MainThreadMarker::from(&*window);
    let backdrop = install_backdrop(mtm, &window);
    let _ = BACKDROP.set(backdrop);
    tracing::debug!(
        "已绑定面板窗口 #{}，背景材质：{backdrop:?}，不激活应用：{}",
        window.windowNumber(),
        NONACTIVATING.load(Ordering::Relaxed)
    );
    PANEL.with(|panel| *panel.borrow_mut() = Some(window));
    Ok(())
}

/// 在内容视图下方插入系统材质作为窗口背景：macOS 26 起是液态玻璃（`NSGlassEffectView`），
/// 更早的系统是 `NSVisualEffectView` 毛玻璃。
///
/// 不能替换 contentView（winit 依赖它），所以放在窗口框架视图里、位于内容视图之下；
/// 窗口本身是透明的，egui 只画半透明的底色，背后的材质就能透出来。
fn install_backdrop(mtm: MainThreadMarker, window: &NSWindow) -> Backdrop {
    let Some(content) = window.contentView() else {
        return Backdrop::Opaque;
    };
    // SAFETY: 只读取父视图引用。
    let Some(frame_view) = (unsafe { content.superview() }) else {
        return Backdrop::Opaque;
    };
    // 旧系统上没有这个类，直接 alloc 会因找不到类而 panic，先在运行时查一下。
    let (view, backdrop): (Retained<NSView>, _) = if AnyClass::get(c"NSGlassEffectView").is_some() {
        let glass =
            NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), content.frame());
        glass.setStyle(NSGlassEffectViewStyle::Regular);
        glass.setCornerRadius(CORNER_RADIUS);
        (Retained::into_super(glass), Backdrop::Glass)
    } else {
        let effect =
            NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), content.frame());
        effect.setMaterial(NSVisualEffectMaterial::Popover);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        effect.setWantsLayer(true);
        // SAFETY: setWantsLayer(true) 之后 layer 是 CALayer，这几个方法都存在
        // （`cornerCurve` 从 macOS 10.15 起才有，先确认）。
        unsafe {
            let layer: Option<Retained<AnyObject>> = msg_send![&*effect, layer];
            if let Some(layer) = layer {
                let _: () = msg_send![&*layer, setCornerRadius: CORNER_RADIUS];
                let _: () = msg_send![&*layer, setMasksToBounds: true];
                let curve = sel!(setCornerCurve:);
                let responds: bool = msg_send![&*layer, respondsToSelector: curve];
                if responds {
                    // `kCACornerCurveContinuous` 的值就是这个字符串。
                    let continuous = NSString::from_str("continuous");
                    let _: () = msg_send![&*layer, setCornerCurve: &*continuous];
                }
            }
        }
        (Retained::into_super(effect), Backdrop::Vibrancy)
    };
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    frame_view.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, Some(&content));
    set_window_corner_radius(window);
    window.invalidateShadow();
    backdrop
}

/// 告诉系统窗口的圆角。
///
/// macOS 26 给前台（key）窗口画的阴影和描边按窗口自己的圆角生成，不看窗口内容的形状；
/// 无边框窗口的这个值是系统默认的小圆角，面板在前台时四角外会露出一圈方形的阴影边。
/// 没有公开 API，用私有的 `_setCornerRadius:`，旧系统上没有这个方法时什么也不做。
fn set_window_corner_radius(window: &NSWindow) {
    let selector = sel!(_setCornerRadius:);
    // SAFETY: 先确认方法存在；参数是 CGFloat。
    unsafe {
        let responds: bool = msg_send![window, respondsToSelector: selector];
        if responds {
            let _: () = msg_send![window, _setCornerRadius: CORNER_RADIUS];
        }
    }
}

/// 收起启动时被 eframe 显示出来的（透明）窗口，并恢复不透明度。
pub fn finish_startup_hide() {
    with_panel(|_, window| {
        window.orderOut(None);
        window.setAlphaValue(1.0);
    });
}

/// 窗口背景用的是哪种材质（UI 据此决定底色的不透明度）。
pub fn backdrop() -> Backdrop {
    BACKDROP.get().copied().unwrap_or_default()
}

/// 在主线程上对已绑定的面板窗口执行 `f`；不在主线程或还没绑定时返回 `None`。
fn with_panel<R>(f: impl FnOnce(MainThreadMarker, &NSWindow) -> R) -> Option<R> {
    let mtm = MainThreadMarker::new()?;
    PANEL.with(|panel| panel.borrow().as_ref().map(|window| f(mtm, window)))
}

/// 面板是否正在显示且是键盘窗口（面板不激活本应用，不能看本应用是否在前台）。
pub fn is_active() -> bool {
    with_panel(|_, window| window.isVisible() && window.isKeyWindow()).unwrap_or(false)
}

/// 面板窗口是否可见（不论是否有焦点）。
pub fn is_visible() -> bool {
    with_panel(|_, window| window.isVisible()).unwrap_or(false)
}

/// 从标题栏空白处开始拖动面板。返回 false 表示面板尚未绑定，由调用方交给 winit。
///
/// winit 的 `drag_window` 直接把 `NSApp.currentEvent` 交给 `performWindowDragWithEvent:`，
/// 系统按这个事件在窗口中的位置计算拖动起点。从菜单栏图标弹出面板后第一次点击面板时，
/// 当前事件可能是窗口激活产生的系统事件而不是这次按下，起点算错，窗口就跳到了鼠标处。
/// 这里只在当前事件确实是本窗口的左键按下 / 拖动时直接使用，否则按鼠标的实际位置构造一个按下事件；
/// 鼠标已经松开（只是单击）时不拖动。
pub fn start_drag() -> bool {
    with_panel(|mtm, window| {
        let current = NSApplication::sharedApplication(mtm).currentEvent();
        let usable = current.as_deref().filter(|event| {
            let kind = event.r#type();
            (kind == NSEventType::LeftMouseDown || kind == NSEventType::LeftMouseDragged)
                && event.windowNumber() == window.windowNumber()
        });
        if let Some(event) = usable {
            window.performWindowDragWithEvent(event);
            return;
        }
        tracing::debug!(
            "拖动面板：当前事件 {:?} 不是本窗口的鼠标按下，按鼠标位置重建",
            current.as_ref().map(|event| event.r#type())
        );
        if NSEvent::pressedMouseButtons() & 1 == 0 {
            return;
        }
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseDown,
            window.mouseLocationOutsideOfEventStream(),
            NSEventModifierFlags::empty(),
            current.map_or(0.0, |event| event.timestamp()),
            window.windowNumber(),
            None,
            0,
            1,
            1.0,
        );
        if let Some(event) = event {
            window.performWindowDragWithEvent(&event);
        }
    })
    .is_some()
}

/// 显示面板并让它成为键盘窗口（不激活本应用）。`size` 是内容区大小（逻辑像素）：同一个窗口既用作剪贴板面板，
/// 也用作鼠标中键快捷面板和 JSON 编辑器，各自大小不同。
/// 窗口原本隐藏、或者大小有变化时，按 `anchor` 重新定位。
pub fn show(anchor: Anchor, size: [f32; 2]) -> bool {
    with_panel(|mtm, window| {
        let visible = window.isVisible();
        // 面板不激活本应用，前台应用就是粘贴目标；面板固定显示时用户可能已经换了应用，每次都更新。
        if let Some(app) = workspace::frontmost_other_app() {
            PREVIOUS_APP.with(|prev| *prev.borrow_mut() = Some(app));
        }
        if resize(window, size) || !visible {
            match anchor {
                Anchor::Placement(placement) => position(mtm, window, placement),
                Anchor::StatusItem(_) => position_below_status_item(mtm, window),
            }
        }
        window.setAlphaValue(1.0);
        if !NONACTIVATING.load(Ordering::Relaxed) {
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        }
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

/// 把内容区改成 `size`（逻辑像素），左上角不动。返回大小是否有变化。
fn resize(window: &NSWindow, size: [f32; 2]) -> bool {
    let frame = window.frame();
    let content = window.contentRectForFrameRect(frame);
    let (width, height) = (f64::from(size[0]), f64::from(size[1]));
    if (content.size.width - width).abs() < 0.5 && (content.size.height - height).abs() < 0.5 {
        return false;
    }
    let top = frame.origin.y + frame.size.height;
    window.setContentSize(NSSize::new(width, height));
    window.setFrameTopLeftPoint(NSPoint::new(frame.origin.x, top));
    true
}

/// 隐藏面板；`restore_focus` 为 true 时确保弹出面板之前的前台应用拿回键盘焦点。
pub fn hide(restore_focus: bool) {
    with_panel(|mtm, window| {
        window.orderOut(None);
        PANEL_KEY.store(false, Ordering::Relaxed);
        let previous = PREVIOUS_APP.with(|prev| prev.borrow_mut().take());
        let app = NSApplication::sharedApplication(mtm);
        tracing::debug!(
            "隐藏面板，恢复焦点：{restore_focus}，本应用在前台：{}",
            app.isActive()
        );
        // 本应用没有被激活时，前台应用一直没变，面板收起后键盘焦点自动回到它。
        if !restore_focus || !app.isActive() {
            return;
        }
        match previous {
            Some(previous) if !previous.isTerminated() => {
                previous.activateWithOptions(NSApplicationActivationOptions::empty());
            }
            // 找不到之前的应用时，隐藏自身让系统激活下一个应用。
            _ => app.hide(None),
        }
    });
}

/// 在鼠标位置弹出原生右键菜单，菜单关闭后才返回；返回是否选中了菜单项。
pub fn popup_menu(menu: &tray_icon::menu::Menu) -> bool {
    use tray_icon::menu::ContextMenu as _;
    // 先取出视图再弹出：菜单跟踪期间会继续绘制界面，不能一直借用着 PANEL。
    let Some(view) = with_panel(|_, window| window.contentView()).flatten() else {
        return false;
    };
    // SAFETY: view 是面板窗口的内容视图，这里持有它的引用，菜单弹出期间一直有效。
    unsafe { menu.show_context_menu_for_nsview(Retained::as_ptr(&view).cast(), None) }
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

/// 按出现位置摆放窗口，并限制在鼠标所在屏幕的可见区域内；
/// “上次的位置”保持不动，只有窗口已不在任何屏幕上时才改到屏幕中央。
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
            NSSize::new(24.0, 24.0),
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

/// 点是否在矩形内（含边界）。
fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x <= rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y <= rect.origin.y + rect.size.height
}

/// 两个矩形是否相交。
fn intersects(a: NSRect, b: NSRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}
