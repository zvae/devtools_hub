//! 全局鼠标中键监听。
//!
//! uTools 的原生插件（`addon/darwin-arm64.node`）在 `VoicePanelTriggerEvent` 中调用
//! `CGEventTapCreate(kCGHIDEventTap, kCGHeadInsertEventTap, kCGEventTapOptionDefault,
//! OtherMouseDown | OtherMouseUp | OtherMouseDragged, otherMouseDownEventCallback, …)`；
//! 回调用 `CGEventGetIntegerValueField(event, kCGMouseEventButtonNumber)` 判断是哪个按键，
//! 处理后返回 NULL，把这次按下和松开吞掉，前台应用收不到中键点击；
//! 监听被系统停用（`kCGEventTapDisabledByTimeout`）时重新 `CGEventTapEnable`。这里是同样的实现。
//!
//! 能拦截事件的 event tap 需要“辅助功能”权限（与自动粘贴相同，uTools 也是在
//! `isHadPrivilege()` 为真时才启用面板）；这里定期检查，用户授权后不用重启即可生效。

use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::Duration;

// 用到的系统类型、函数与常量，名字与 Apple 头文件中的一致。
type CFTypeRef = *const c_void;
type CGEventRef = *mut c_void;
type CFMachPortRef = *mut c_void;
type TapCallback = unsafe extern "C" fn(
    proxy: *mut c_void,
    kind: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: TapCallback,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: CFTypeRef,
        port: CFMachPortRef,
        order: isize,
    ) -> CFTypeRef;
    fn CFRunLoopGetCurrent() -> CFTypeRef;
    fn CFRunLoopAddSource(run_loop: CFTypeRef, source: CFTypeRef, mode: CFTypeRef);
    fn CFRunLoopRun();
    static kCFRunLoopCommonModes: CFTypeRef;
}

const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
/// 主动拦截（可以修改或丢弃事件），对应 uTools 传入的 0。
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
const K_CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const K_CG_EVENT_OTHER_MOUSE_UP: u32 = 26;
const K_CG_EVENT_OTHER_MOUSE_DRAGGED: u32 = 27;
const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
const K_CG_MOUSE_EVENT_BUTTON_NUMBER: u32 = 3;
/// uTools 传入的事件掩码 `0xe000000`：OtherMouseDown / Up / Dragged 三位。
const EVENT_MASK: u64 = (1 << K_CG_EVENT_OTHER_MOUSE_DOWN)
    | (1 << K_CG_EVENT_OTHER_MOUSE_UP)
    | (1 << K_CG_EVENT_OTHER_MOUSE_DRAGGED);
/// 按键号：0 左键、1 右键、2 中键、3 / 4 侧键。
const MIDDLE_BUTTON: i64 = 2;
/// 没有权限或创建监听失败时，多久再试一次。
const RETRY_INTERVAL: Duration = Duration::from_secs(3);

/// 传给事件回调的上下文（泄漏成静态引用，回调随时可能被调用）。
struct Context {
    /// 创建好的 event tap，被系统停用时用它重新启用。
    tap: AtomicPtr<c_void>,
    /// 这次中键按下已被处理（吞掉），对应的拖动和松开也一并吞掉。
    swallowing: AtomicBool,
    on_press: Box<dyn Fn() -> bool + Send + Sync>,
}

/// 启动监听线程。中键按下时在监听线程上调用 `on_press`（必须很快返回），
/// 返回 true 表示已处理，这次点击不再传给前台应用。
pub fn spawn_middle_click_listener(on_press: impl Fn() -> bool + Send + Sync + 'static) {
    let context: &'static Context = Box::leak(Box::new(Context {
        tap: AtomicPtr::new(ptr::null_mut()),
        swallowing: AtomicBool::new(false),
        on_press: Box::new(on_press),
    }));
    std::thread::Builder::new()
        .name("middle-click".into())
        .spawn(move || run(context))
        .expect("无法创建鼠标中键监听线程");
}

/// 监听线程：有权限后创建 event tap 并运行 run loop；没有权限时定期重试。
fn run(context: &'static Context) {
    let mut warned = false;
    loop {
        // 先静默检查权限：没有权限时不去创建，避免反复触发系统的授权提示。
        let trusted = super::keyboard::accessibility_trusted(false);
        // SAFETY: 回调签名与 CGEventTapCallBack 一致；context 是泄漏的静态引用，始终有效。
        let tap = if !trusted {
            ptr::null_mut()
        } else {
            unsafe {
                CGEventTapCreate(
                    K_CG_HID_EVENT_TAP,
                    K_CG_HEAD_INSERT_EVENT_TAP,
                    K_CG_EVENT_TAP_OPTION_DEFAULT,
                    EVENT_MASK,
                    callback,
                    ptr::from_ref(context).cast_mut().cast(),
                )
            }
        };
        if tap.is_null() {
            if !warned {
                tracing::warn!("无法监听鼠标中键：需要“辅助功能”权限，授权后会自动生效");
                warned = true;
            }
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        }
        context.tap.store(tap, Ordering::SeqCst);
        tracing::info!("开始监听鼠标中键");
        // SAFETY: tap 是刚创建的有效 CFMachPort；run loop 属于当前线程，CFRunLoopRun 不会返回。
        unsafe {
            let source = CFMachPortCreateRunLoopSource(ptr::null(), tap, 0);
            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
            CGEventTapEnable(tap, true);
            CFRunLoopRun();
        }
        return;
    }
}

/// SAFETY: 由 CoreGraphics 在监听线程上调用；`user_info` 是 [`spawn_middle_click_listener`]
/// 中泄漏的 `Context`。
unsafe extern "C" fn callback(
    _proxy: *mut c_void,
    kind: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    let context = unsafe { &*user_info.cast::<Context>() };
    let is_middle = || unsafe {
        CGEventGetIntegerValueField(event, K_CG_MOUSE_EVENT_BUTTON_NUMBER) == MIDDLE_BUTTON
    };
    match kind {
        K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT | K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT => {
            let tap = context.tap.load(Ordering::SeqCst);
            if !tap.is_null() {
                unsafe { CGEventTapEnable(tap, true) };
            }
            event
        }
        K_CG_EVENT_OTHER_MOUSE_DOWN if is_middle() => {
            // 回调跨越 FFI 边界，不能让 panic 传出去。
            let handled = std::panic::catch_unwind(AssertUnwindSafe(|| (context.on_press)()))
                .unwrap_or(false);
            context.swallowing.store(handled, Ordering::SeqCst);
            if handled {
                ptr::null_mut()
            } else {
                event
            }
        }
        K_CG_EVENT_OTHER_MOUSE_DRAGGED
            if is_middle() && context.swallowing.load(Ordering::SeqCst) =>
        {
            ptr::null_mut()
        }
        K_CG_EVENT_OTHER_MOUSE_UP
            if is_middle() && context.swallowing.swap(false, Ordering::SeqCst) =>
        {
            ptr::null_mut()
        }
        _ => event,
    }
}
