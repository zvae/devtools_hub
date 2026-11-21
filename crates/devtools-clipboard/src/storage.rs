//! 可复用的持久化文件写入：同目录临时文件、同步、原子替换。

#[cfg(unix)]
use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    pending.write_all(bytes)?;
    pending.as_file().sync_all()?;
    pending.persist(path).map_err(|e| e.error)?;
    sync_directory(parent)
}

/// Unix 上同步目录项；Windows 的普通文件句柄不能用于同步目录。
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
