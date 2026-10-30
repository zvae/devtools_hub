//! 平台层的文件系统来源：`std::fs` 实现（§7.3 扫描时预取元数据）。
//!
//! 扫描时一次 `read_dir` + `DirEntry::metadata`（lstat 语义，不跟随符号链接），
//! 大小/修改时间/创建时间在列表项里带出，查询阶段不再 stat。
//! 已知限制（v1）：
//! - 创建时间：macOS 的 `st_birthtime` 没有 std 接口，为 `Unknown`（排序置后、显示"—"）；
//!   Windows 用 `Metadata::created()`；
//! - 设备号：Unix 用 `MetadataExt::dev()`；Windows 依赖卷 GUID 判卷（见 `volumes`）。

use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

use devtools_search_core::ports::{Entry, EntryKind, FsSource};
use devtools_search_core::NodeMeta;

/// `std::fs` 实现的 FsSource。
#[derive(Clone, Copy, Debug, Default)]
pub struct StdFs;

impl FsSource for StdFs {
    fn list_dir(&self, path: &Path, out: &mut Vec<Entry>) -> io::Result<()> {
        out.clear();
        let dir = match std::fs::read_dir(path) {
            Ok(dir) => dir,
            Err(err) => return Err(err),
        };
        for item in dir {
            let item = match item {
                Ok(item) => item,
                // 单个目录项的失败不影响整目录（§7.3）。
                Err(_) => continue,
            };
            let name = item.file_name();
            // `file_type()` 是 lstat 语义：符号链接不被跟随。
            let file_type = match item.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let kind = if file_type.is_symlink() {
                EntryKind::Symlink
            } else if file_type.is_dir() {
                EntryKind::Dir
            } else if file_type.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            let mut meta = NodeMeta::default();
            if !file_type.is_symlink() {
                if let Ok(metadata) = item.metadata() {
                    if kind == EntryKind::File {
                        meta.size = Some(metadata.len());
                    }
                    meta.mtime = metadata.modified().ok().and_then(to_unix);
                    meta.btime = metadata.created().ok().and_then(to_unix);
                }
            }
            out.push(Entry { name, kind, meta });
        }
        Ok(())
    }

    fn device_of(&self, path: &Path) -> Option<u64> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // 用 symlink_metadata（lstat），避免跟随符号链接到别的卷。
            std::fs::symlink_metadata(path).ok().map(|m| m.dev())
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
    }
}

fn to_unix(time: std::time::SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devtools_search_core::ports::FsSource;

    #[test]
    fn lists_directory_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut entries = Vec::new();
        StdFs.list_dir(dir.path(), &mut entries).unwrap();
        assert_eq!(entries.len(), 2);
        let file = entries
            .iter()
            .find(|e| e.name == "a.txt")
            .expect("a.txt 应被列出");
        assert_eq!(file.kind, EntryKind::File);
        assert_eq!(file.meta.size, Some(5));
        assert!(file.meta.mtime.is_some());
        let sub = entries.iter().find(|e| e.name == "sub").unwrap();
        assert_eq!(sub.kind, EntryKind::Dir);
        assert_eq!(sub.meta.size, None, "目录不记录大小");
    }

    #[test]
    fn reports_io_error_for_missing_directory() {
        let mut entries = Vec::new();
        assert!(StdFs.list_dir(Path::new("/definitely/not/here"), &mut entries).is_err());
    }
}
