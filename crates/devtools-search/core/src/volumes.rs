//! 根生命周期状态与 root_key 身份规则（§7.5）。
//!
//! `RootLocation` 的身份 = `volume_id + relative_path`；绝对路径仅用于展示和没有稳定
//! 卷身份时的定位。根不写入 CoreStorage（宿主主数据库）。

use sha2::{Digest, Sha256};

use crate::model::RootLocation;

/// 根生命周期状态（§7.5 状态机）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootState {
    /// 尚未启用。
    Dormant,
    /// 等待权限/同意（首次扫描引导）。
    AwaitingConsent,
    /// 卷未挂载。
    Offline,
    /// 挂载回归后验证路径与配置指纹。
    PendingValidation,
    /// 全量扫描中。
    Scanning,
    /// 验证/合并事件流（监听 catch-up）。
    Verifying,
    /// 就绪。
    Ready,
    /// 监听落后，需要局部/全量验证。
    Dirty,
    /// 用户暂停。
    Paused,
    /// 卷已拔出，正在冻结保存。
    Ejecting,
    /// 出错（错误信息）。
    Error(String),
}

impl RootState {
    pub fn label(&self) -> &'static str {
        match self {
            RootState::Dormant => "未启用",
            RootState::AwaitingConsent => "等待权限",
            RootState::Offline => "卷未挂载",
            RootState::PendingValidation => "验证中",
            RootState::Scanning => "扫描中",
            RootState::Verifying => "更新验证中",
            RootState::Ready => "就绪",
            RootState::Dirty => "更新可能滞后",
            RootState::Paused => "已暂停",
            RootState::Ejecting => "卷已拔出，保存中",
            RootState::Error(_) => "错误",
        }
    }
}

/// `root_key`：SHA-256（带格式版本的规范编码 `(volume_id, relative_path)`）的完整十六进制。
///
/// 统一使用可序列化的长度前缀编码（postcard），不拼接易混淆的字符串（§7.5）。
pub fn root_key(location: &RootLocation) -> String {
    const FORMAT_VERSION: &str = "devtools-search-root-key-v1";
    let encoded = postcard::to_allocvec(&(
        FORMAT_VERSION,
        &location.volume_id.0,
        &location.relative_path,
    ))
    .expect("root_key 编码不会失败");
    let digest = Sha256::digest(&encoded);
    hex(&digest)
}

/// 校验新根与已有根的冲突：同 volume 下相同路径为重复；互为父/子目录为嵌套（§6.3）。
pub fn validate_new_root(new: &RootLocation, existing: &[RootLocation]) -> Result<(), RootConflict> {
    for old in existing {
        if old.volume_id != new.volume_id {
            continue;
        }
        let new_path = new.relative_path.trim_matches('/');
        let old_path = old.relative_path.trim_matches('/');
        if new_path == old_path {
            return Err(RootConflict::Duplicate);
        }
        let new_is_parent = old_path.starts_with(&format!("{new_path}/")) || new_path.is_empty();
        let old_is_parent = new_path.starts_with(&format!("{old_path}/")) || old_path.is_empty();
        if new_is_parent || old_is_parent {
            return Err(RootConflict::Nested);
        }
    }
    Ok(())
}

/// 根冲突类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootConflict {
    Duplicate,
    Nested,
}

impl std::fmt::Display for RootConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RootConflict::Duplicate => write!(f, "该目录已在索引根中"),
            RootConflict::Nested => write!(f, "与已有索引根存在父子嵌套，请先调整已有根"),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::VolumeId;
    use std::path::PathBuf;

    fn loc(volume: &str, rel: &str) -> RootLocation {
        RootLocation {
            volume_id: VolumeId(volume.into()),
            relative_path: rel.into(),
            fallback_absolute_path: PathBuf::from(format!("/{rel}")),
        }
    }

    #[test]
    fn root_key_is_stable_and_distinguishes_roots() {
        let a = loc("vol-a", "Users/me");
        assert_eq!(root_key(&a), root_key(&a));
        assert_eq!(root_key(&a).len(), 64);
        assert_ne!(root_key(&a), root_key(&loc("vol-a", "Users/you")));
        assert_ne!(root_key(&a), root_key(&loc("vol-b", "Users/me")));
        // 绝对路径变化不影响身份（换挂载点）。
        let mut moved = a.clone();
        moved.fallback_absolute_path = PathBuf::from("/Volumes/Disk 1/Users/me");
        assert_eq!(root_key(&a), root_key(&moved));
    }

    #[test]
    fn validates_duplicate_and_nested_roots() {
        let home = loc("v", "Users/me");
        assert_eq!(validate_new_root(&home, &[home.clone()]), Err(RootConflict::Duplicate));
        assert_eq!(
            validate_new_root(&loc("v", "Users/me/Documents"), &[home.clone()]),
            Err(RootConflict::Nested)
        );
        assert_eq!(
            validate_new_root(&home, &[loc("v", "Users/me/Documents")]),
            Err(RootConflict::Nested)
        );
        assert_eq!(validate_new_root(&loc("v", "Users/you"), &[home]), Ok(()));
        // 卷根与任何子目录嵌套。
        assert_eq!(
            validate_new_root(&loc("v", ""), &[loc("v", "Users")]),
            Err(RootConflict::Nested)
        );
        // 不同卷不冲突。
        assert_eq!(validate_new_root(&loc("w", "Users/me"), &[loc("v", "Users/me")]), Ok(()));
    }
}
