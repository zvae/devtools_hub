//! 平台层：扫描、卷与监听适配（§3.2、§8）。

pub mod fs;
pub mod volumes;
pub mod watcher;
#[cfg(any(target_os = "macos", windows))]
#[cfg_attr(target_os = "macos", path = "fsevents.rs")]
#[cfg_attr(windows, path = "usn.rs")]
pub(crate) mod journal;
