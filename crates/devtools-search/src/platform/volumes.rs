//! 卷身份与挂载枚举（§7.5、§8）。
//!
//! - Windows：`GetVolumeNameForVolumeMountPointW` 给出卷 GUID（稳定身份）；
//! - macOS / 其他 Unix：内核 `st_dev` 作为卷标识（稳定于同一挂载周期；跨重新挂载可能变化，
//!   这是 M0 待验证项——稳定 UUID 需要 DiskArbitration/`statfs`，不在本轮实现）。
//! 拿不到稳定身份时用显式标记的绝对路径兜底（`VolumeId::fallback_absolute`）。

use std::path::Path;

use devtools_search_core::ports::{VolumeMount, VolumeSource};
use devtools_search_core::VolumeId;

/// `std` 实现的卷来源。
#[derive(Clone, Copy, Debug, Default)]
pub struct StdVolumes;

impl VolumeSource for StdVolumes {
    fn current_mounts(&self) -> Vec<VolumeMount> {
        let mut mounts = Vec::new();
        #[cfg(unix)]
        {
            mounts.push(mount_id(Path::new("/")));
            // macOS：/Volumes 下的每一项通常是一个挂载点。
            if let Ok(entries) = std::fs::read_dir("/Volumes") {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if is_mount_point(&path) {
                        mounts.push(mount_id(&path));
                    }
                }
            }
        }
        #[cfg(windows)]
        {
            use std::path::PathBuf;

            for letter in b'A'..=b'Z' {
                let root = PathBuf::from(format!("{}:\\", letter as char));
                if root.exists() {
                    if let Some(id) = windows_volume_id(&root) {
                        mounts.push(VolumeMount {
                            volume_id: id,
                            mount_point: root,
                        });
                    }
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            mounts.push(mount_id(Path::new("/")));
        }
        mounts
    }

    fn volume_id_of(&self, path: &Path) -> Option<VolumeId> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(path).ok().map(|m| VolumeId(format!("unix-dev:{}", m.dev())))
        }
        #[cfg(windows)]
        {
            windows_volume_id(path)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Some(VolumeId::fallback_absolute(path))
        }
    }
}

#[cfg(not(windows))]
fn mount_id(mount_point: &Path) -> VolumeMount {
    let volume_id = StdVolumes
        .volume_id_of(mount_point)
        .unwrap_or_else(|| VolumeId::fallback_absolute(mount_point));
    VolumeMount {
        volume_id,
        mount_point: mount_point.to_path_buf(),
    }
}

/// 判断是否是挂载点：挂载点的 `st_dev` 与其父目录不同。
#[cfg(unix)]
fn is_mount_point(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(path), path.parent().map(std::fs::metadata)) {
        (Ok(own), Some(Ok(parent))) => own.dev() != parent.dev(),
        _ => false,
    }
}

/// Windows：用盘符根（`C:\`）作为卷身份。
///
/// 稳定的卷 GUID 需要 `GetVolumeNameForVolumeMountPointW`，`windows-sys` 0.61 没有导出它；
/// 盘符根对本地盘足够稳定（跨重启不变），换盘符的情况极少，先按这个实现，留作 v2 升级点。
#[cfg(windows)]
fn windows_volume_id(path: &Path) -> Option<VolumeId> {
    use std::path::Component;
    match path.components().next()? {
        Component::Prefix(prefix) => Some(VolumeId(format!(
            "win-drive:{}",
            prefix.as_os_str().to_string_lossy()
        ))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_mounts_carry_stable_ids() {
        let mounts = StdVolumes.current_mounts();
        assert!(!mounts.is_empty(), "至少应枚举出一个卷");
        // 枚举出的挂载点再问一次身份，必须与枚举时一致。
        for mount in &mounts {
            let again = StdVolumes.volume_id_of(&mount.mount_point).expect("已枚举的卷必须可再识别");
            assert_eq!(again, mount.volume_id);
        }
    }

    #[cfg(unix)]
    #[test]
    fn identifies_root_volume() {
        let id = StdVolumes.volume_id_of(Path::new("/")).expect("根卷可识别");
        assert!(!id.0.is_empty());
        // 同一卷上两个路径得到同一身份。
        let a = StdVolumes.volume_id_of(Path::new("/tmp")).unwrap();
        let b = StdVolumes.volume_id_of(Path::new("/usr")).unwrap();
        assert_eq!(a, b);
    }

    #[cfg(unix)]
    #[test]
    fn root_is_in_current_mounts() {
        let mounts = StdVolumes.current_mounts();
        assert!(mounts.iter().any(|m| m.mount_point == Path::new("/")));
    }

    #[cfg(windows)]
    #[test]
    fn drive_roots_are_in_current_mounts() {
        let mounts = StdVolumes.current_mounts();
        assert!(mounts.iter().all(|m| m.volume_id.0.starts_with("win-drive:")));
        // 当前工作目录所在盘必须在枚举结果里。
        let cwd = std::env::current_dir().expect("有工作目录");
        let cwd_id = StdVolumes.volume_id_of(&cwd).expect("工作目录所在卷可识别");
        assert!(mounts.iter().any(|m| m.volume_id == cwd_id));
    }
}
