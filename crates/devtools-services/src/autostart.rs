use anyhow::{Context, Result};
use std::env;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{fs, path::PathBuf};

#[cfg(target_os = "macos")]
const APPLICATION_ID: &str = "com.devtoolshub.app";

/// Enable or disable launching the current executable at user login.
pub fn set_enabled(enabled: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        return set_macos(enabled);
    }

    #[cfg(target_os = "windows")]
    {
        return set_windows(enabled);
    }

    #[cfg(target_os = "linux")]
    {
        return set_linux(enabled);
    }

    #[allow(unreachable_code)]
    Err(anyhow::anyhow!(
        "autostart is not supported on this platform"
    ))
}

#[cfg(target_os = "macos")]
fn set_macos(enabled: bool) -> Result<()> {
    let home = env::var_os("HOME").context("HOME is not set")?;
    let path = PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{APPLICATION_ID}.plist"));

    if !enabled {
        return remove_if_exists(&path);
    }

    let executable = env::current_exe().context("failed to locate current executable")?;
    let parent = path.parent().context("invalid launch agent path")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let contents = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\"><dict>\n\
<key>Label</key><string>{APPLICATION_ID}</string>\n\
<key>ProgramArguments</key><array><string>{}</string></array>\n\
<key>RunAtLoad</key><true/>\n\
</dict></plist>\n",
        xml_escape(&executable.to_string_lossy())
    );
    fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_linux(enabled: bool) -> Result<()> {
    let config_dir = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .context("neither XDG_CONFIG_HOME nor HOME is set")?;
    let path = config_dir.join("autostart").join("devtools-hub.desktop");

    if !enabled {
        return remove_if_exists(&path);
    }

    let executable = env::current_exe().context("failed to locate current executable")?;
    let parent = path.parent().context("invalid desktop entry path")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName=DevTools Hub\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        desktop_exec_escape(&executable.to_string_lossy())
    );
    fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn set_windows(enabled: bool) -> Result<()> {
    use windows::{
        core::PCWSTR,
        Win32::System::Registry::{
            RegCloseKey, RegCreateKeyW, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW,
            HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ,
        },
    };

    let key_name: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0"
        .encode_utf16()
        .collect();
    let value_name: Vec<u16> = "DevTools Hub\0".encode_utf16().collect();
    let mut key = windows::Win32::System::Registry::HKEY::default();
    let open_result = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_name.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut key,
        )
    };

    if !enabled {
        if open_result.is_err() {
            return Ok(());
        }
        let result = unsafe { RegDeleteValueW(key, PCWSTR(value_name.as_ptr())) }.ok();
        let _ = unsafe { RegCloseKey(key) };
        return result.map_err(|error| anyhow::anyhow!(error.to_string()));
    }

    if open_result.is_err() {
        unsafe { RegCreateKeyW(HKEY_CURRENT_USER, PCWSTR(key_name.as_ptr()), &mut key) }
            .ok()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    }

    let executable = env::current_exe().context("failed to locate current executable")?;
    let value: Vec<u16> = format!("\"{}\"\0", executable.to_string_lossy())
        .encode_utf16()
        .collect();
    let result = unsafe {
        RegSetValueExW(
            key,
            PCWSTR(value_name.as_ptr()),
            Some(0u32),
            REG_SZ,
            Some(std::slice::from_raw_parts(
                value.as_ptr() as *const u8,
                value.len() * 2,
            )),
        )
    };
    let _ = unsafe { RegCloseKey(key) };
    result
        .ok()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn remove_if_exists(path: &PathBuf) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "linux")]
fn desktop_exec_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(' ', "\\s")
        .replace('"', "\\\"")
}
