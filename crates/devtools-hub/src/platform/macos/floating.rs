//! 截屏遮罩与贴图窗口的原生设置。
//!
//! 两种窗口都由 egui 的多视口创建（eframe 不提供子视口的原生句柄），这里按窗口标题找到对应的 `NSWindow` 再调整：
//! - 截屏遮罩：层级 `NSScreenSaverWindowLevel`，盖住菜单栏和程序坞，出现在当前桌面（含全屏应用所在的桌面）。
//!   与 Snipaste 一样激活本应用（日志 “Snipper: focus in”）：后台应用不能改变鼠标指针，
//!   不激活就显示不了十字光标和调整大小的箭头。结束后把焦点还给之前的应用。
//! - 贴图：Snipaste 的贴图是 `Qt::Tool` 窗口，即 `QNSPanel`（它还替换了 `QNSPanel` 的 `setLevel:`，
//!   让贴图保持置顶）。这里同样换成 `NSPanel` 子类，并且不激活应用：点击、拖动贴图时前台应用不变，
//!   贴图仍能成为键盘窗口，响应 esc、⌘C 等按键——但只在被鼠标点中时（见 [`PinPanel`]）。

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{define_class, ClassType, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSEvent, NSEventType, NSModalResponseOK,
    NSNormalWindowLevel, NSOpenPanel, NSPanel, NSResponder, NSRunningApplication, NSSavePanel,
    NSScreen, NSScreenSaverWindowLevel, NSWindow, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowLevel,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};

use super::{panel, workspace};

/// 贴图窗口是否支持调整不透明度。
pub const OPACITY_SUPPORTED: bool = true;

thread_local! {
    /// 截屏前的前台应用，截屏结束后把焦点还给它（只在主线程访问）。
    static PREVIOUS_APP: RefCell<Option<Retained<NSRunningApplication>>> = const { RefCell::new(None) };
    /// 截屏结束时正在还焦点的应用（见 [`focus_restored`]）。
    static RESTORING_APP: RefCell<Option<Retained<NSRunningApplication>>> = const { RefCell::new(None) };
    /// 贴图窗口换类前原来的类（winit 的 `WinitWindow`），销毁前换回去（见 [`release_pin`]）。
    static PIN_ORIGINAL_CLASS: Cell<Option<&'static AnyClass>> = const { Cell::new(None) };
    /// 正在拖动的贴图窗口与按住的那一点相对窗口左上角的位置（点，见 [`start_drag`]）。
    static DRAG: RefCell<Option<(Retained<NSWindow>, NSPoint)>> = const { RefCell::new(None) };
}

define_class!(
    /// 贴图窗口的类：绑定时把 winit 的 `WinitWindow` 实例换成这个类（两者都没有实例变量）。
    ///
    /// SAFETY: `NSPanel` 可以被继承；这里没有实例变量，也不需要 `Drop`。
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DevToolsHubPin"]
    struct PinPanel;

    impl PinPanel {
        /// 只在鼠标点中这张贴图时才能成为键盘窗口。
        ///
        /// 面板、截屏遮罩收起时，AppKit 会在本应用剩下的窗口里挑一个接任键盘窗口；贴图是不激活应用的面板，
        /// 被挑中后即使前台还是别的应用，按键也都发给贴图——实测有贴图时选中剪贴板条目，
        /// 模拟的 ⌘V 发给了贴图，没有粘贴到应用里。所以只有当前事件是落在这张贴图上的鼠标按下时才答应。
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            NSApplication::sharedApplication(self.mtm())
                .currentEvent()
                .is_some_and(|event| {
                    let kind = event.r#type();
                    let mouse_down = kind == NSEventType::LeftMouseDown
                        || kind == NSEventType::RightMouseDown
                        || kind == NSEventType::OtherMouseDown;
                    mouse_down && event.windowNumber() == self.windowNumber()
                })
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }

        /// 不把贴图限制在菜单栏下面：比屏幕还高的长截图贴图要能往上移，放大时窗口也要能伸到屏幕外。
        #[unsafe(method(constrainFrameRect:toScreen:))]
        fn constrain_frame_rect(&self, frame: NSRect, _screen: Option<&NSScreen>) -> NSRect {
            frame
        }
    }
);

/// 正在显示的截屏遮罩的标题（[`rescue_overlay`] 在其他线程上用到）。
static OVERLAY: Mutex<Option<String>> = Mutex::new(None);

unsafe extern "C" {
    /// 主队列（`dispatch_get_main_queue()` 是返回它的宏）。
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

/// 按标题找到本应用的窗口（只在主线程可用）。
fn find(title: &str) -> Option<(MainThreadMarker, Retained<NSWindow>)> {
    let mtm = MainThreadMarker::new()?;
    let title = NSString::from_str(title);
    NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .find(|window| window.title().isEqualToString(&title))
        .map(|window| (mtm, window))
}

/// 把 CG 的全局坐标（点，主显示器左上角为原点，y 向下）换成 AppKit 的坐标（主显示器左下角为原点，y 向上）。
fn to_cocoa(mtm: MainThreadMarker, frame: [f64; 4]) -> NSRect {
    let main_height = objc2_app_kit::NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |screen| screen.frame().size.height);
    let [x, y, width, height] = frame;
    NSRect::new(
        NSPoint::new(x, main_height - y - height),
        NSSize::new(width, height),
    )
}

/// 遮罩窗口创建时是否隐藏。macOS 上遮罩是透明窗口，创建时什么都不显示，直接显示即可（Windows 见那里的说明）。
pub const OVERLAY_STARTS_HIDDEN: bool = false;

/// 只在遮罩隐藏着创建时调用（macOS 上不会）。
pub fn show_overlay(_title: &str, _frame: [f64; 4]) -> bool {
    true
}

/// 遮罩画出第一帧后调用；macOS 上没有要做的。
pub fn reveal_overlay(_title: &str) {}

/// 截屏遮罩出现后调用：盖住整个显示器（`frame` 是 CG 全局坐标）、提到菜单栏之上，并激活本应用接收按键。
/// 返回是否找到了窗口（窗口还没创建时下一帧再试）。
pub fn begin_overlay(title: &str, frame: [f64; 4]) -> bool {
    let Some((mtm, window)) = find(title) else {
        return false;
    };
    window.setLevel(NSScreenSaverWindowLevel);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.setHasShadow(false);
    // 关掉 AppKit 显示、收起窗口时的缩放淡出动画：遮罩画的是整屏的静止截图，
    // 收起时缩小、半透明地叠在真实屏幕上，看起来整屏模糊了一下（录屏里约 100ms）。
    window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    // 无边框窗口不受菜单栏限制，可以直接盖住整个屏幕。
    window.setFrame_display(to_cocoa(mtm, frame), true);
    *OVERLAY.lock().unwrap() = Some(title.to_owned());
    if let Some(app) = workspace::frontmost_other_app() {
        PREVIOUS_APP.with(|prev| *prev.borrow_mut() = Some(app));
    }
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    window.makeKeyAndOrderFront(None);
    window.orderFrontRegardless();
    true
}

/// 截屏遮罩是透明窗口：滚动截图时选区里看到的就是真实的屏幕，鼠标在选区里时让整个遮罩穿透。
pub const OVERLAY_TRANSPARENT: bool = true;

/// 进入滚动截图时调用：返回遮罩的窗口编号（`CGWindowID`），截取选区时用来把遮罩排除在截图之外。
/// 窗口还没创建时为空。选区外的遮罩由界面直接画在透明窗口上，用不到 `_mask`。
pub fn begin_scroll(title: &str, _mask: egui::Color32) -> Option<u32> {
    let (_, window) = find(title)?;
    u32::try_from(window.windowNumber())
        .ok()
        .filter(|id| *id != 0)
}

/// 遮罩是透明窗口、用鼠标穿透，不需要改变窗口的形状（见 Windows 版）。
pub fn set_scroll_regions(_title: &str, _hole: egui::Rect, _shown: &[(egui::Rect, f32)]) {}

/// 透明窗口结束时什么都不画即可（见 Windows 版）。
pub fn end_scroll(_title: &str) {}

/// 收起截屏遮罩（egui 随后销毁窗口）。
pub fn hide_overlay(title: &str) {
    {
        let mut overlay = OVERLAY.lock().unwrap();
        if overlay.as_deref() == Some(title) {
            *overlay = None;
        }
    }
    match find(title) {
        Some((_, window)) => window.orderOut(None),
        None => tracing::warn!("没找到截屏遮罩“{title}”"),
    }
}

/// 截屏遮罩卡住时由看门狗调用（任意线程）：收起遮罩、把焦点还给截屏前的应用。
///
/// 投递到主队列执行，不经过 eframe：卡住时 eframe 可能不再处理事件，但主线程的 run loop 还在运行
/// （主队列在模态对话框等嵌套的 run loop 里也会执行）。
pub fn rescue_overlay() {
    extern "C" fn work(_: *mut c_void) {
        let title = OVERLAY.lock().unwrap().clone();
        if let Some(title) = title {
            hide_overlay(&title);
        }
        restore_previous_app();
    }
    // SAFETY: `_dispatch_main_q` 是 libdispatch 导出的主队列；`work` 不使用 context。
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, std::ptr::null_mut(), work) };
}

/// 截屏结束：把焦点还给截屏前的应用。
///
/// 截屏期间本应用是前台，原来的应用处于非活跃状态（窗口的侧栏、标题栏变灰），只是被静止的截图挡住了。
/// 先收起遮罩再还焦点，中间会露出一帧整屏的非活跃外观，看起来像屏幕闪了一下；
/// 所以先调用这里，等原来的应用恢复（[`focus_restored`]）后再 [`hide_overlay`]。
pub fn restore_focus() {
    let previous = restore_previous_app();
    RESTORING_APP.with(|app| *app.borrow_mut() = previous);
}

/// [`restore_focus`] 还焦点的应用是否已经回到前台（没有要还的应用时为 true）。
///
/// 激活是异步的，有的应用（例如 Electron 应用）要好几百毫秒才恢复，固定等一段时间不够。
pub fn focus_restored() -> bool {
    RESTORING_APP.with(|app| {
        app.borrow()
            .as_ref()
            .is_none_or(|app| app.isTerminated() || app.isActive())
    })
}

/// 激活截屏前（或保存对话框前）的前台应用，返回它。
fn restore_previous_app() -> Option<Retained<NSRunningApplication>> {
    let previous = PREVIOUS_APP.with(|prev| prev.borrow_mut().take());
    let previous = previous.filter(|app| !app.isTerminated())?;
    previous.activateWithOptions(NSApplicationActivationOptions::empty());
    Some(previous)
}

/// 贴图窗口出现后调用：换成不激活应用的面板，显示阴影，留在创建它的桌面上。返回是否找到了窗口。
pub fn configure_pin(title: &str) -> bool {
    let Some((_, window)) = find(title) else {
        return false;
    };
    let original = panel::original_class(&window);
    if panel::make_nonactivating(&window, PinPanel::class()) {
        PIN_ORIGINAL_CLASS.set(Some(original));
    }
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::FullScreenAuxiliary | NSWindowCollectionBehavior::IgnoresCycle,
    );
    // NSPanel 默认在应用失去激活时隐藏；贴图本来就不激活应用，必须关掉。
    window.setHidesOnDeactivate(false);
    window.setHasShadow(true);
    true
}

/// 贴图关闭、窗口销毁前调用：把窗口换回原来的类，系统移除它对窗口的观察时才不会抛异常（见 `panel::swap_class`）。
pub fn release_pin(title: &str) {
    let (Some((_, window)), Some(original)) = (find(title), PIN_ORIGINAL_CLASS.get()) else {
        return;
    };
    if !window.isKindOfClass(PinPanel::class()) {
        return;
    }
    window.orderOut(None);
    panel::swap_class(&window, original);
}

/// 一次设置窗口的位置和大小（`frame` 是全局坐标 `[x, y, 宽, 高]`，点）。分成 egui 的 `InnerSize` 和 `OuterPosition`
/// 两个命令时，AppKit 先按左上角不动改变大小、再移动，连续缩放贴图时窗口会抖动。返回 false 表示没找到窗口。
pub fn set_window_frame(title: &str, frame: [f64; 4]) -> bool {
    let Some((mtm, window)) = find(title) else {
        return false;
    };
    window.setFrame_display(to_cocoa(mtm, frame), true);
    true
}

/// 设置贴图窗口的不透明度（0–1）。
pub fn set_opacity(title: &str, opacity: f32) {
    if let Some((_, window)) = find(title) {
        window.setAlphaValue(f64::from(opacity));
    }
}

/// 显示 / 隐藏贴图窗口的阴影。
pub fn set_shadow(title: &str, shadow: bool) {
    if let Some((_, window)) = find(title) {
        window.setHasShadow(shadow);
    }
}

/// 按住鼠标拖动贴图窗口：之后每帧由 [`drag_moved`] 让窗口跟着鼠标移动，松开时 [`end_drag`]。
/// `grab` 是按下鼠标的位置（相对窗口左上角，点）：开始拖动时鼠标已经移动了一小段，
/// 按这个位置算，按住的那一点始终在鼠标下面。
///
/// 不用系统的拖动（`performWindowDragWithEvent`，主面板仍然用它）：系统让窗口保持在菜单栏下面，
/// 比屏幕还高的长截图贴图就拖不上去了。返回 false 表示没找到窗口，由调用方交给 winit。
pub fn start_drag(title: &str, grab: egui::Vec2) -> bool {
    let Some((_, window)) = find(title) else {
        return false;
    };
    let grab = NSPoint::new(f64::from(grab.x), f64::from(grab.y));
    DRAG.with(|drag| *drag.borrow_mut() = Some((window, grab)));
    drag_moved();
    true
}

/// 拖动中：移动窗口，让按住的那一点回到鼠标下面。
pub fn drag_moved() {
    DRAG.with(|drag| {
        let drag = drag.borrow();
        let Some((window, grab)) = drag.as_ref() else {
            return;
        };
        // AppKit 的坐标原点在主显示器左下角、y 向上；`grab` 从窗口左上角往下量。
        let mouse = NSEvent::mouseLocation();
        let height = window.frame().size.height;
        window.setFrameOrigin(NSPoint::new(mouse.x - grab.x, mouse.y + grab.y - height));
    });
}

/// 松开鼠标，结束拖动。
pub fn end_drag() {
    DRAG.with(|drag| drag.borrow_mut().take());
}

/// 在鼠标位置弹出贴图的右键菜单，菜单关闭后才返回。
pub fn popup_menu(title: &str, menu: &tray_icon::menu::Menu) -> bool {
    let Some(view) = find(title).and_then(|(_, window)| window.contentView()) else {
        return false;
    };
    panel::popup_menu_in(&view, menu)
}

/// 弹出保存对话框，返回选择的路径（自动补上 `.png`）；取消时返回 `None`。
///
/// 本应用平时不在前台（菜单栏应用、贴图不激活应用），对话框要能输入文件名，先激活本应用，
/// 关闭后再按 `restore_focus` 决定要不要把焦点还给它前面的应用：
/// - 截屏、贴图用它，还回去之后用户继续用原来的应用；
/// - 从面板（提取文字）里弹的时候不还：面板还要接着用，而且“还焦点”是异步的，
///   晚一步落地就会把面板的键盘焦点抢走，界面把这一下当成“用户点了别的应用”，窗口就收起了。
///
/// 面板（层级 101）与截屏遮罩 / 贴图（层级 1000）都浮在普通窗口之上，对话框会被它们盖住、点不到，
/// 所以弹出期间先把这些窗口降到普通层级，关闭后恢复。
pub fn save_dialog(
    file_name: &str,
    directory: Option<&Path>,
    restore_focus: bool,
) -> Option<PathBuf> {
    let mtm = MainThreadMarker::new()?;
    if let Some(app) = workspace::frontmost_other_app() {
        PREVIOUS_APP.with(|prev| *prev.borrow_mut() = Some(app));
    }
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    let dialog = NSSavePanel::savePanel(mtm);
    dialog.setNameFieldStringValue(&NSString::from_str(file_name));
    dialog.setCanCreateDirectories(true);
    dialog.setExtensionHidden(false);
    if let Some(url) = directory
        .and_then(|dir| dir.to_str())
        .map(|dir| NSURL::fileURLWithPath(&NSString::from_str(dir)))
    {
        dialog.setDirectoryURL(Some(&url));
    }
    let lowered = lower_floating_windows(mtm);
    DIALOG_OPEN.store(true, Ordering::Relaxed);
    let response = dialog.runModal();
    DIALOG_OPEN.store(false, Ordering::Relaxed);
    restore_levels(lowered);
    let path = (response == NSModalResponseOK)
        .then(|| dialog.URL())
        .flatten()
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()));
    if restore_focus {
        restore_previous_app();
    }
    path.map(crate::snip::ensure_png)
}

/// 把本应用那些浮在普通层之上的窗口（面板、截屏遮罩、贴图）降到普通层级，
/// 返回原来的层级以便 [`restore_levels`] 恢复。菜单栏图标的窗口不降，否则图标会跟着消失。
fn lower_floating_windows(mtm: MainThreadMarker) -> Vec<(Retained<NSWindow>, NSWindowLevel)> {
    let app = NSApplication::sharedApplication(mtm);
    let mut lowered = Vec::new();
    for window in app.windows().iter() {
        let level = window.level();
        let class = window.class().name().to_bytes().to_vec();
        if level <= NSNormalWindowLevel || class.starts_with(b"NSStatusBarWindow") {
            continue;
        }
        window.setLevel(NSNormalWindowLevel);
        lowered.push((window.clone(), level));
    }
    tracing::debug!("保存对话框：临时降低 {} 个窗口的层级", lowered.len());
    lowered
}

/// 恢复 [`lower_floating_windows`] 降下来的层级。
fn restore_levels(lowered: Vec<(Retained<NSWindow>, NSWindowLevel)>) {
    for (window, level) in lowered {
        window.setLevel(level);
    }
}

/// 保存对话框是否正在显示（见 [`save_dialog`]）。
static DIALOG_OPEN: AtomicBool = AtomicBool::new(false);

/// 是否正在显示本应用弹出的系统对话框。对话框打开期间面板失去焦点是正常的，
/// 界面层据此不要自动收起面板（见 `HubApp::handle_focus`）。
pub fn dialog_open() -> bool {
    DIALOG_OPEN.load(Ordering::Relaxed)
}

/// 选择文件夹（文件搜索模块的索引根）。取消返回 `Ok(None)`，失败返回带诊断的错误。
///
/// 与 [`save_dialog`] 一样：先激活本应用（对话框要能接收键盘输入），弹出期间把浮层降到普通层级，
/// 关闭后恢复，最后把焦点还给之前的应用。
pub fn pick_folder(initial: Option<&std::path::Path>) -> Result<Option<std::path::PathBuf>, String> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err("不在主线程，无法打开文件夹选择对话框".into());
    };
    if let Some(app) = workspace::frontmost_other_app() {
        PREVIOUS_APP.with(|prev| *prev.borrow_mut() = Some(app));
    }
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    let dialog = NSOpenPanel::openPanel(mtm);
    dialog.setCanChooseDirectories(true);
    dialog.setCanChooseFiles(false);
    dialog.setAllowsMultipleSelection(false);
    dialog.setCanCreateDirectories(true);
    dialog.setResolvesAliases(true);
    dialog.setMessage(Some(&NSString::from_str("选择要建立索引的文件夹")));
    dialog.setPrompt(Some(&NSString::from_str("选择")));
    if let Some(url) = initial
        .and_then(|dir| dir.to_str())
        .map(|dir| NSURL::fileURLWithPath(&NSString::from_str(dir)))
    {
        dialog.setDirectoryURL(Some(&url));
    }
    let lowered = lower_floating_windows(mtm);
    DIALOG_OPEN.store(true, Ordering::Relaxed);
    let response = dialog.runModal();
    DIALOG_OPEN.store(false, Ordering::Relaxed);
    restore_levels(lowered);
    let path = (response == NSModalResponseOK)
        .then(|| dialog.URL())
        .flatten()
        .and_then(|url| url.path())
        .map(|path| std::path::PathBuf::from(path.to_string()));
    restore_previous_app();
    Ok(path)
}

thread_local! {
    /// 系统模态期间被降到普通层级的窗口（只在主线程访问，见 [`begin_system_modal`]）。
    static MODAL_LOWERED: RefCell<Vec<(Retained<NSWindow>, NSWindowLevel)>> = const { RefCell::new(Vec::new()) };
}

/// 搜索模块的原生模态开始：激活本应用、降低浮层层级并标记"对话框已打开"，
/// 使面板不会因为失焦被自动收起（§0：模态保护由宿主实现）。
pub fn begin_system_modal() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    if let Some(app) = workspace::frontmost_other_app() {
        PREVIOUS_APP.with(|prev| *prev.borrow_mut() = Some(app));
    }
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    let lowered = lower_floating_windows(mtm);
    MODAL_LOWERED.with(|slot| *slot.borrow_mut() = lowered);
    DIALOG_OPEN.store(true, Ordering::Relaxed);
    true
}

/// 原生模态结束：恢复层级与对话框标记；`restore_focus` 为 true 时把焦点还给之前的应用。
pub fn end_system_modal(restore_focus: bool) {
    DIALOG_OPEN.store(false, Ordering::Relaxed);
    MODAL_LOWERED.with(|slot| {
        let lowered: Vec<_> = slot.borrow_mut().drain(..).collect();
        restore_levels(lowered);
    });
    if restore_focus {
        restore_previous_app();
    }
}
