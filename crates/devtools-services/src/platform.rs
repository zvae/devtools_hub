pub fn active_application_name() -> Option<String> {
    active_window_title()
}

#[cfg(target_os = "windows")]
fn active_window_title() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    };

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

#[cfg(not(target_os = "windows"))]
fn active_window_title() -> Option<String> {
    None
}
