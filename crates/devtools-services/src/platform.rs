/// 返回当前活动应用/窗口名称。现阶段 Windows 返回窗口标题，其它平台先占位。
pub fn active_application_name() -> Option<String> {
    active_window_title()
}

/// Windows 通过 Win32 API 读取前台窗口标题。
#[cfg(target_os = "windows")]
fn active_window_title() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    };

    // Win32 调用只能通过 unsafe 进入，后续立即校验句柄和返回长度。
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }

    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return None;
    }

    let mut buffer = vec![0u16; len as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    if copied <= 0 {
        return None;
    }

    let title = String::from_utf16_lossy(&buffer[..copied as usize])
        .trim()
        .to_string();
    (!title.is_empty()).then_some(title)
}

/// 非 Windows 平台暂未实现活动窗口识别，避免错误猜测来源应用。
#[cfg(not(target_os = "windows"))]
fn active_window_title() -> Option<String> {
    None
}
