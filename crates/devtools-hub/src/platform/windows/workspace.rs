//! 来源程序与系统打开操作。
//!
//! Windows 没有 Bundle ID，这里用程序的完整路径作为标识（`SourceApp::bundle_id`），
//! 名称取自程序文件的“文件说明”（如 “Visual Studio Code”），取不到时用文件名。

use std::ffi::c_void;
use std::path::Path;
use std::ptr::{null, null_mut};

use devtools_clipboard::SourceApp;
use windows_sys::Win32::Foundation::{CloseHandle, HWND};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows_sys::Win32::System::DataExchange::GetClipboardOwner;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, SW_SHOWNORMAL,
};

use super::wide;

/// 写入剪贴板的程序：优先取剪贴板所有者窗口，取不到（所有者为空）时用前台窗口。
pub fn frontmost_app() -> SourceApp {
    // SAFETY: 两个查询函数都没有参数。
    let owner = unsafe { GetClipboardOwner() };
    let window = if owner.is_null() {
        unsafe { GetForegroundWindow() }
    } else {
        owner
    };
    describe_window(window)
}

/// 窗口所属的程序。
pub fn describe_window(window: HWND) -> SourceApp {
    if window.is_null() {
        return SourceApp::default();
    }
    let mut pid = 0u32;
    // SAFETY: pid 指向有效的栈变量。
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    let Some(path) = process_path(pid) else {
        return SourceApp::default();
    };
    let name = file_description(&path).unwrap_or_else(|| {
        Path::new(&path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone())
    });
    SourceApp {
        bundle_id: Some(path),
        name: Some(name),
    }
}

fn process_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    // SAFETY: 句柄在使用后立即关闭；缓冲区长度通过 size 传入。
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = vec![0u16; 1024];
        let mut size = buffer.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut size);
        CloseHandle(process);
        (ok != 0).then(|| String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

/// 程序文件版本信息中的“文件说明”。
fn file_description(path: &str) -> Option<String> {
    let path = wide(path);
    // SAFETY: 版本信息块由我们分配并在函数内使用；VerQueryValueW 返回的指针指向该块内部。
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(path.as_ptr(), &mut handle);
        if size == 0 {
            return None;
        }
        let mut block = vec![0u8; size as usize];
        if GetFileVersionInfoW(path.as_ptr(), 0, size, block.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let query = |sub_block: &str| -> Option<(*mut c_void, u32)> {
            let mut pointer: *mut c_void = null_mut();
            let mut len = 0u32;
            let sub_block = wide(sub_block);
            let ok = VerQueryValueW(
                block.as_ptr().cast(),
                sub_block.as_ptr(),
                &mut pointer,
                &mut len,
            );
            (ok != 0 && !pointer.is_null() && len > 0).then_some((pointer, len))
        };
        // 先用文件声明的语言与代码页，再试英文和简体中文。
        let mut translations = Vec::new();
        if let Some((pointer, len)) = query(r"\VarFileInfo\Translation") {
            if len >= 4 {
                let pair = pointer.cast::<u16>();
                translations.push((*pair, *pair.add(1)));
            }
        }
        translations.extend([(0x0409, 0x04B0), (0x0804, 0x04B0)]);
        translations.into_iter().find_map(|(language, code_page)| {
            let key = format!(r"\StringFileInfo\{language:04x}{code_page:04x}\FileDescription");
            let (pointer, len) = query(&key)?;
            let units = std::slice::from_raw_parts(pointer.cast::<u16>(), len as usize);
            let text = String::from_utf16_lossy(units);
            let text = text.trim_end_matches('\0').trim();
            (!text.is_empty()).then(|| text.to_string())
        })
    }
}

fn shell_open(file: &str, parameters: Option<&str>) -> bool {
    let (operation, file) = (wide("open"), wide(file));
    let parameters = parameters.map(wide);
    // SAFETY: 所有字符串都以 NUL 结尾，且在调用期间有效。
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ref().map_or(null(), |p| p.as_ptr()),
            null(),
            SW_SHOWNORMAL,
        )
    };
    // 返回值大于 32 表示成功。
    result as usize > 32
}

pub fn open_url(url: &str) -> bool {
    shell_open(url, None)
}

/// 在资源管理器中选中文件。
pub fn reveal_path(path: &str) {
    shell_open("explorer.exe", Some(&format!("/select,\"{path}\"")));
}

pub fn open_path(path: &Path) {
    shell_open(&path.to_string_lossy(), None);
}
