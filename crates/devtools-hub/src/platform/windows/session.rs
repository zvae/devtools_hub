//! 系统会话退出监听：独立线程上的隐藏顶层窗口接收关机、重启、注销及 Restart Manager 通知。
//!
//! 不能用仅消息窗口，它收不到会话广播；也不能在 UI 线程等待 UI 自己完成清理。
//! 查询阶段立即同意，只有 WM_ENDSESSION 确认结束时才请求退出并等待收尾。

use std::cell::RefCell;
use std::io;
use std::ptr::{null, null_mut};
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    PostQuitMessage, RegisterClassW, TranslateMessage, MSG, WM_CLOSE, WM_DESTROY, WM_ENDSESSION,
    WM_QUERYENDSESSION, WNDCLASSW,
};

thread_local! {
    static ON_END: RefCell<Option<Box<dyn Fn()>>> = RefCell::new(None);
}

pub struct SessionListener {
    window: usize,
    worker: Option<JoinHandle<()>>,
}

impl SessionListener {
    pub fn start(on_end: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("system-session".into())
            .spawn(move || {
                ON_END.with(|handler| *handler.borrow_mut() = Some(Box::new(on_end)));
                // SAFETY: 窗口和消息循环都属于本线程；类名缓冲区在创建窗口前有效。
                unsafe {
                    let class = super::wide("DevToolsHub.SessionListener");
                    let instance = GetModuleHandleW(null());
                    let description = WNDCLASSW {
                        lpfnWndProc: Some(window_proc),
                        hInstance: instance,
                        lpszClassName: class.as_ptr(),
                        ..std::mem::zeroed()
                    };
                    if RegisterClassW(&description) == 0
                        && GetLastError() != ERROR_CLASS_ALREADY_EXISTS
                    {
                        let _ = tx.send(Err(io::Error::last_os_error()));
                        return;
                    }
                    // parent 为 NULL，且不使用 WS_VISIBLE，创建隐藏的顶层窗口。
                    let hwnd = CreateWindowExW(
                        0,
                        class.as_ptr(),
                        class.as_ptr(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        null_mut(),
                        null_mut(),
                        instance,
                        null(),
                    );
                    if hwnd.is_null() {
                        let _ = tx.send(Err(io::Error::last_os_error()));
                        return;
                    }
                    if tx.send(Ok(hwnd as usize)).is_err() {
                        DestroyWindow(hwnd);
                        return;
                    }
                    let mut message: MSG = std::mem::zeroed();
                    while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                ON_END.with(|handler| handler.borrow_mut().take());
            })?;
        let window = rx
            .recv()
            .map_err(|_| io::Error::other("系统退出监听线程启动失败"))??;
        Ok(Self {
            window,
            worker: Some(worker),
        })
    }
}

impl Drop for SessionListener {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            if !worker.is_finished() {
                // SAFETY: 窗口只由本监听器持有；实际销毁在其所属线程的 WM_CLOSE 中完成。
                unsafe {
                    PostMessageW(self.window as HWND, WM_CLOSE, 0, 0);
                }
            }
            let _ = worker.join();
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wparam != 0 {
                // 取消关机（wParam=FALSE）时不得停止后台任务。防止 panic 穿过原生回调边界。
                let result = std::panic::catch_unwind(|| {
                    ON_END.with(|handler| {
                        if let Some(on_end) = handler.borrow().as_ref() {
                            on_end();
                        }
                    })
                });
                if result.is_err() {
                    tracing::error!("处理系统退出通知失败");
                }
            }
            0
        }
        WM_CLOSE => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    #[test]
    fn only_confirmed_session_end_requests_cleanup() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let listener = SessionListener::start(move || {
            count.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        // 只向本测试自己的窗口发消息，不会触发真实关机或注销。
        unsafe {
            let hwnd = listener.window as HWND;
            assert_eq!(SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0), 1);
            SendMessageW(hwnd, WM_ENDSESSION, 0, 0);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            SendMessageW(hwnd, WM_ENDSESSION, 1, 0);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
}
