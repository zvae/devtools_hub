//! 全局鼠标中键监听（Windows）。
//!
//! 用低级鼠标钩子 `SetWindowsHookExW(WH_MOUSE_LL)`，在单独的线程上跑消息循环。
//! 与 macOS 版相同：中键按下交给 `on_press` 决定是否处理，处理了就返回非零值把这次
//! 按下和松开吞掉，前台程序收不到中键点击。钩子回调必须很快返回
//! （超过 `LowLevelHooksTimeout` 系统会跳过并最终卸载钩子），所以只转发事件，不做实际工作。

use std::mem::zeroed;
use std::panic::AssertUnwindSafe;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, HC_ACTION, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT,
    WH_MOUSE_LL, WM_MBUTTONDOWN, WM_MBUTTONUP,
};

/// 中键按下时的回调，返回是否已处理。
type Handler = Box<dyn Fn() -> bool + Send + Sync>;

/// 低级鼠标钩子是全局回调，回调函数只能放在静态变量里。
static ON_PRESS: OnceLock<Handler> = OnceLock::new();
/// 这次中键按下已被处理（吞掉），对应的松开也一并吞掉。
static SWALLOWING: AtomicBool = AtomicBool::new(false);

/// 启动监听线程。中键按下时在监听线程上调用 `on_press`（必须很快返回），
/// 返回 true 表示已处理，这次点击不再传给前台程序。
pub fn spawn_middle_click_listener(on_press: impl Fn() -> bool + Send + Sync + 'static) {
    if ON_PRESS.set(Box::new(on_press)).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("middle-click".into())
        .spawn(|| {
            // SAFETY: 钩子过程在整个进程生命周期内有效；消息循环保持钩子线程存活。
            unsafe {
                let hook =
                    SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), GetModuleHandleW(null()), 0);
                if hook.is_null() {
                    tracing::warn!("无法监听鼠标中键：SetWindowsHookExW 失败");
                    return;
                }
                tracing::info!("开始监听鼠标中键");
                let mut message: MSG = zeroed();
                while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {}
            }
        })
        .expect("无法创建鼠标中键监听线程");
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: WH_MOUSE_LL 的 lparam 指向 MSLLHOOKSTRUCT。
        let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        // 程序模拟的点击（例如自动化工具）不处理。
        let injected = info.flags & LLMHF_INJECTED != 0;
        match wparam as u32 {
            WM_MBUTTONDOWN if !injected => {
                let handled = ON_PRESS.get().is_some_and(|on_press| {
                    std::panic::catch_unwind(AssertUnwindSafe(on_press)).unwrap_or(false)
                });
                SWALLOWING.store(handled, Ordering::SeqCst);
                if handled {
                    return 1;
                }
            }
            WM_MBUTTONUP if SWALLOWING.swap(false, Ordering::SeqCst) => return 1,
            _ => {}
        }
    }
    // SAFETY: 按约定把其余事件交给下一个钩子。
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}
