//! Windows 剪贴板读写。
//!
//! 对应 macOS `changeCount` 的是 `GetClipboardSequenceNumber`：剪贴板内容每变化一次加一，
//! 读取它不需要打开剪贴板，适合轮询。剪贴板格式与 UTI 之间的转换见
//! `devtools_clipboard::windows_formats`。

use std::ptr::null_mut;
use std::time::Duration;

use devtools_clipboard::windows_formats::{self, ClipboardFormat, FormatData, FormatTarget};
use devtools_clipboard::Snapshot;
use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};

use super::wide;

/// 系统剪贴板。Windows 没有命名剪贴板，所以只有一个实例。
pub struct Pasteboard;

impl Pasteboard {
    /// 系统剪贴板。
    pub fn general() -> Self {
        Self
    }

    /// Windows 没有 macOS 那样的命名剪贴板，`--pasteboard` 参数被忽略。
    pub fn named(name: &str) -> Self {
        tracing::warn!("Windows 不支持命名剪贴板，忽略 --pasteboard {name}，改用系统剪贴板");
        Self
    }

    /// 剪贴板的修改序号，每次写入都会变；监听线程据此发现新内容。
    pub fn change_count(&self) -> isize {
        // SAFETY: 无参数的查询函数。
        unsafe { GetClipboardSequenceNumber() as isize }
    }

    /// 只读取格式列表（以及隐私标记的值），用于在读取大块数据之前先做隐私判断。
    pub fn types(&self) -> Vec<String> {
        let Some(_guard) = ClipboardGuard::open(null_mut()) else {
            return Vec::new();
        };
        windows_formats::type_names(&enumerate(false))
    }

    /// 读取剪贴板上的所有格式；剪贴板被占用时返回空内容。
    pub fn read(&self) -> Snapshot {
        let Some(_guard) = ClipboardGuard::open(null_mut()) else {
            tracing::debug!("剪贴板被其他程序占用，本次跳过");
            return Snapshot::default();
        };
        windows_formats::snapshot_from_windows(&enumerate(true))
    }

    /// 写入快照，返回写入后的序号（调用方据此让监听线程跳过这次自己的写入）。
    pub fn write(&self, snapshot: &Snapshot) -> isize {
        let formats = windows_formats::windows_formats_from_snapshot(snapshot);
        // 用面板窗口作为剪贴板所有者；以 NULL 打开时 EmptyClipboard 会把所有者设为空，
        // 某些系统上随后的 SetClipboardData 会失败。
        match ClipboardGuard::open(super::panel::window()) {
            Some(_guard) => {
                // SAFETY: 剪贴板已由本线程打开。
                unsafe { EmptyClipboard() };
                for format in formats {
                    set_format(&format);
                }
            }
            None => tracing::warn!("剪贴板被其他程序占用，写入失败"),
        }
        self.change_count()
    }
}

/// 打开剪贴板，离开作用域时自动关闭。其他程序可能正占用剪贴板，短暂重试。
struct ClipboardGuard;

impl ClipboardGuard {
    /// 打开剪贴板，被占用时每隔一小段时间重试，最多 20 次。
    fn open(owner: HWND) -> Option<Self> {
        for _ in 0..20 {
            // SAFETY: owner 为面板窗口或空指针。
            if unsafe { OpenClipboard(owner) } != 0 {
                return Some(Self);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: 只有成功打开后才会创建 ClipboardGuard。
        unsafe { CloseClipboard() };
    }
}

/// 枚举剪贴板上的格式。`read_all` 为 false 时只读取隐私标记的数据（类型判断用）。
/// 调用前必须已打开剪贴板。
fn enumerate(read_all: bool) -> Vec<ClipboardFormat> {
    let mut formats = Vec::new();
    let mut id = 0;
    loop {
        // SAFETY: 剪贴板已打开；传入上一个格式编号继续枚举。
        id = unsafe { EnumClipboardFormats(id) };
        if id == 0 {
            break;
        }
        let name = format_name(id);
        if read_all {
            if !windows_formats::should_read(id) {
                continue;
            }
            if let Some(data) = read_data(id) {
                formats.push(ClipboardFormat { id, name, data });
            }
        } else {
            let is_marker = name.as_deref() == Some(windows_formats::CAN_INCLUDE_IN_HISTORY);
            let data = if is_marker {
                read_data(id).unwrap_or_default()
            } else {
                Vec::new()
            };
            formats.push(ClipboardFormat { id, name, data });
        }
    }
    formats
}

/// 注册格式（编号 ≥ 0xC000）的名称；标准格式返回 `None`。
fn format_name(id: u32) -> Option<String> {
    if id < 0xC000 {
        return None;
    }
    let mut buffer = [0u16; 256];
    // SAFETY: 缓冲区长度与传入的最大长度一致。
    let len = unsafe { GetClipboardFormatNameW(id, buffer.as_mut_ptr(), buffer.len() as i32) };
    (len > 0).then(|| String::from_utf16_lossy(&buffer[..len as usize]))
}

/// 读取一种格式的内存块。调用前必须已打开剪贴板。
fn read_data(id: u32) -> Option<Vec<u8>> {
    // SAFETY: 剪贴板已打开；句柄只在锁定期间访问，长度取自 GlobalSize。
    unsafe {
        let handle = GetClipboardData(id);
        if handle.is_null() {
            return None;
        }
        let pointer = GlobalLock(handle);
        if pointer.is_null() {
            return None;
        }
        let size = GlobalSize(handle);
        let data = std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec();
        GlobalUnlock(handle);
        Some(data)
    }
}

/// 写入一种格式。调用前必须已打开并清空剪贴板。
fn set_format(format: &FormatData) {
    // SAFETY: 分配的内存写满后交给系统；SetClipboardData 失败时自行释放。
    unsafe {
        let id = match &format.target {
            FormatTarget::Standard(id) => *id,
            FormatTarget::Registered(name) => RegisterClipboardFormatW(wide(name).as_ptr()),
        };
        if id == 0 {
            return;
        }
        let memory = GlobalAlloc(GMEM_MOVEABLE, format.data.len().max(1));
        if memory.is_null() {
            return;
        }
        let pointer = GlobalLock(memory).cast::<u8>();
        if pointer.is_null() {
            GlobalFree(memory);
            return;
        }
        std::ptr::copy_nonoverlapping(format.data.as_ptr(), pointer, format.data.len());
        GlobalUnlock(memory);
        if SetClipboardData(id, memory).is_null() {
            GlobalFree(memory);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use devtools_clipboard::{uti, Representation};

    /// 会改写系统剪贴板，默认忽略：cargo test -p devtools-hub clipboard_round_trip -- --ignored
    #[test]
    #[ignore = "会改写系统剪贴板"]
    fn clipboard_round_trip() {
        let pasteboard = Pasteboard::general();
        let snapshot = Snapshot {
            items: vec![vec![
                Representation::new(uti::PLAIN_TEXT, "你好，Windows"),
                Representation::new(uti::HTML, "<b>你好</b>"),
            ]],
        };
        let before = pasteboard.change_count();
        let after = pasteboard.write(&snapshot);
        assert_ne!(before, after);
        let read = pasteboard.read();
        assert_eq!(uti::plain_text(&read).as_deref(), Some("你好，Windows"));
        assert!(read.has_type(uti::HTML));
        assert!(!pasteboard.types().is_empty());
    }
}
