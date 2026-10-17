//! 开机启动：写入 `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`。

use anyhow::{ensure, Context, Result};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};

use super::wide;

/// 当前用户的开机启动项注册表键，以及本应用的值名。
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "DevToolsHub";

/// 是否已设置开机启动（注册表中有本应用的值）。
pub fn is_enabled() -> bool {
    let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
    // SAFETY: 只查询值是否存在，不读取数据。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    status == ERROR_SUCCESS
}

/// 开启时写入当前可执行文件的路径，关闭时删除该值。
pub fn set_enabled(enabled: bool) -> Result<()> {
    let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
    if enabled {
        let exe = std::env::current_exe().context("无法获取可执行文件路径")?;
        let command = wide(&format!("\"{}\"", exe.display()));
        // SAFETY: 数据是以 NUL 结尾的 UTF-16，长度按字节传入。
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        };
        ensure!(status == ERROR_SUCCESS, "写入注册表失败（错误码 {status}）");
    } else {
        // SAFETY: 字符串以 NUL 结尾。
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        ensure!(
            status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND,
            "删除注册表项失败（错误码 {status}）"
        );
    }
    Ok(())
}
