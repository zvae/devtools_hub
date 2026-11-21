//! 系统右键菜单：`IContextMenu` + `TrackPopupMenuEx`，与资源管理器里右键同一个菜单。
//!
//! 好处是"打开方式""发送到"、Git / 7-Zip / 压缩软件的扩展全都在，不需要我们逐条实现。
//! 在 shell 菜单**前面**再插三条固定项（打开 / 打开路径 / 复制完整路径和文件名）：
//! 这三件事在搜索场景里用得最多，不该每次翻到 shell 菜单里去找。
//!
//! 代价是弹出的是**系统原生菜单**（不在 egui 的渲染里，离屏快照抓不到），
//! 而且它是模态的：`TrackPopupMenuEx` 自己跑消息循环，直到用户选完才返回。
//!
//! 调用发生在 UI 线程上，会让这一帧的绘制推迟到菜单关闭之后；此时屏幕上是上一帧的内容，
//! 菜单本身由系统绘制，交互不受影响（资源管理器也是如此）。

use std::mem::size_of;
use std::path::Path;

use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Foundation::{HANDLE, POINT};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    IContextMenu, IShellItem, SHCreateItemFromParsingName, BHID_SFUIObject, CMF_NORMAL,
    CMINVOKECOMMANDINFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetCursorPos, GetForegroundWindow, InsertMenuW,
    SetForegroundWindow, TrackPopupMenuEx, MF_BYPOSITION, MF_SEPARATOR, MF_STRING, SW_SHOWNORMAL,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN,
};

use super::wide;
use crate::platform::MenuChoice;

/// 我们自己加的固定项的命令 id。shell 用的是 1..0x7FFF，这里从 0x8000 起，互不冲突。
const OWN_OPEN: usize = 0x8000;
const OWN_REVEAL: usize = 0x8001;
const OWN_COPY: usize = 0x8002;

/// 在鼠标位置弹出某个文件的系统右键菜单，顶部带三条固定项。
pub fn show_for_path(path: &Path) -> MenuChoice {
    let wide_path = wide(&path.to_string_lossy());
    // 三条固定项的文字（`InsertMenuW` 要 NUL 结尾的 UTF-16）。
    let open_text = wide("打开");
    let reveal_text = wide("打开路径");
    let copy_text = wide("复制完整路径和文件名");
    // SAFETY: 只调用 Win32/COM 接口；句柄在函数内创建并释放，指针都指向本函数的栈变量。
    unsafe {
        // 菜单在 UI 线程上弹，这里保证该线程有 COM 单元（重复初始化返回 S_FALSE，无害）。
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return MenuChoice::Dismissed;
        }
        let item: IShellItem = match SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None) {
            Ok(item) => item,
            Err(_) => return MenuChoice::Dismissed,
        };
        // `BindToHandler` 要的是 `IShellItem` 的 UI 对象（`BHID_SFUIObject`），
        // 它背后就是资源管理器右键用的 `IContextMenu`。
        let context: IContextMenu = match item.BindToHandler(None, &BHID_SFUIObject) {
            Ok(context) => context,
            Err(_) => return MenuChoice::Dismissed,
        };
        let Ok(menu) = CreatePopupMenu() else {
            return MenuChoice::Dismissed;
        };
        // 1..0x7FFF 这一段 id 留给 shell 自己的命令（"打开方式"等）。
        if context
            .QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL)
            .is_err()
        {
            let _ = DestroyMenu(menu);
            return MenuChoice::Dismissed;
        }
        // 固定项插到最前面（`MF_BYPOSITION` 按位置算，后面的项依次下移），再补一条分隔线。
        // 插入失败也不影响：菜单还是 shell 那份，只是少了顶部三项。
        let _ = InsertMenuW(menu, 0, MF_BYPOSITION | MF_STRING, OWN_OPEN, PCWSTR(open_text.as_ptr()));
        let _ = InsertMenuW(menu, 1, MF_BYPOSITION | MF_STRING, OWN_REVEAL, PCWSTR(reveal_text.as_ptr()));
        let _ = InsertMenuW(menu, 2, MF_BYPOSITION | MF_STRING, OWN_COPY, PCWSTR(copy_text.as_ptr()));
        let _ = InsertMenuW(menu, 3, MF_BYPOSITION | MF_SEPARATOR, 0, PCWSTR::null());

        let mut point = POINT::default();
        if GetCursorPos(&mut point).is_err() {
            let _ = DestroyMenu(menu);
            return MenuChoice::Dismissed;
        }
        // 必须先置前台，否则点菜单外面菜单不会关闭（Win32 的老规矩）。
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD.0 | TPM_RIGHTBUTTON.0 | TPM_LEFTALIGN.0 | TPM_TOPALIGN.0,
            point.x,
            point.y,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        // 返回 0 表示用户点了外面取消。
        let chosen = command.0 as usize;
        let choice = match chosen {
            0 => MenuChoice::Dismissed,
            OWN_OPEN => MenuChoice::Open,
            OWN_REVEAL => MenuChoice::Reveal,
            OWN_COPY => MenuChoice::CopyPath,
            _ => MenuChoice::Handled,
        };
        if choice == MenuChoice::Handled {
            // shell 的命令 id 是 1 起，`lpVerb` 用 MAKEINTRESOURCEA(id - 1)。
            let info = CMINVOKECOMMANDINFO {
                cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32,
                fMask: 0,
                hwnd,
                lpVerb: PCSTR((chosen - 1) as *const u8),
                lpParameters: PCSTR::null(),
                lpDirectory: PCSTR::null(),
                nShow: SW_SHOWNORMAL.0,
                dwHotKey: 0,
                hIcon: HANDLE::default(),
            };
            let _ = context.InvokeCommand(&info);
        }
        choice
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetMenuStringW, MF_BYCOMMAND};

    /// 只走查前半段：拿到 `IContextMenu`、`QueryContextMenu` 能生成菜单项，
    /// 再插我们自己的三项（不弹窗，避免阻塞测试）。
    #[test]
    fn shell_builds_a_context_menu_with_own_items() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("menu.txt");
        std::fs::write(&file, "x").unwrap();
        let wide_path = wide(&file.to_string_lossy());
        // SAFETY: 同 `show_for_path`，只是把最后一步（弹窗）省掉。
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let item: IShellItem =
                SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None).unwrap();
            let context: IContextMenu = item.BindToHandler(None, &BHID_SFUIObject).unwrap();
            let menu = CreatePopupMenu().unwrap();
            context
                .QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL)
                .unwrap();
            let shell_items = GetMenuItemCount(Some(menu));
            assert!(shell_items > 2, "系统菜单至少要有「打开」「属性」这些东西：{shell_items}");

            let open_text = wide("打开");
            let _ = InsertMenuW(menu, 0, MF_BYPOSITION | MF_STRING, OWN_OPEN, PCWSTR(open_text.as_ptr()));
            let count = GetMenuItemCount(Some(menu));
            assert_eq!(count, shell_items + 1, "插入固定项后菜单项数应加一");

            // 第一条就是我们的「打开」，而且是按命令 id 能取到文字的普通项。
            let mut buffer = [0u16; 128];
            let len = GetMenuStringW(menu, OWN_OPEN as u32, Some(&mut buffer), MF_BYCOMMAND);
            assert!(len > 0, "固定项应该有文字");
            let first = String::from_utf16_lossy(&buffer[..len as usize]);
            assert_eq!(first, "打开");
            let _ = DestroyMenu(menu);
        }
    }
}
