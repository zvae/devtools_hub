//! 核心数据类型：ID、根位置、索引配置与查询选项（§7.1、§7.5、§3.1）。
//!
//! 这些类型归 core 定义一份；外层 `settings` 单向依赖这里的纯类型（§3.3）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 根内节点 ID（u32；约 42.9 亿上限，同一 `index_epoch` 内不复用，§7.1）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct NodeId(pub u32);

impl NodeId {
    pub const ROOT: NodeId = NodeId(0);

    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// 名字条目 ID（指向名字 arena 的条目）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NameId(pub u32);

/// 会话内根 ID（u16）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct RootId(pub u16);

impl RootId {
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// 打包 RootId 与 NodeId 的结果行 ID（§7.1：`ResultId=u64` 打包 RootId/NodeId）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResultId(pub u64);

impl ResultId {
    pub fn new(root: RootId, node: NodeId) -> Self {
        Self(((root.0 as u64) << 32) | u64::from(self_node(node)))
    }

    pub fn root(self) -> RootId {
        RootId((self.0 >> 32) as u16)
    }

    pub fn node(self) -> NodeId {
        NodeId(self.0 as u32)
    }
}

fn self_node(node: NodeId) -> u64 {
    u64::from(node.0)
}

/// 卷身份：稳定标识（Windows 卷 GUID / macOS 磁盘 UUID 等）。
/// 无稳定卷身份时用显式标记的规范绝对路径身份兜底（§7.5）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VolumeId(pub String);

impl VolumeId {
    const FALLBACK_PREFIX: &'static str = "path:";

    /// 无稳定卷身份时的兜底身份（明确标记，不与稳定 ID 混淆）。
    pub fn fallback_absolute(path: &Path) -> Self {
        Self(format!("{}{}", Self::FALLBACK_PREFIX, path.display()))
    }

    pub fn is_fallback(&self) -> bool {
        self.0.starts_with(Self::FALLBACK_PREFIX)
    }
}

/// 根位置：卷相对路径 + 兜底绝对路径（§7.5）。
/// 绝对路径仅用于展示和没有稳定卷身份时的定位；身份由 `(volume_id, relative_path)` 决定。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RootLocation {
    pub volume_id: VolumeId,
    /// 相对卷挂载点的路径（`/` 分隔，不以 `/` 开头）。
    pub relative_path: String,
    /// 兜底绝对路径（展示、无稳定卷身份时的定位）。
    pub fallback_absolute_path: PathBuf,
}

impl RootLocation {
    /// 挂载点已知时解析出的绝对路径。
    pub fn absolute(&self, mount: &Path) -> PathBuf {
        if self.relative_path.is_empty() {
            mount.to_path_buf()
        } else {
            mount.join(&self.relative_path)
        }
    }

    /// 展示路径：用兜底绝对路径（保持系统分隔符）。
    pub fn display_path(&self) -> PathBuf {
        self.fallback_absolute_path.clone()
    }
}

/// 一个索引根的配置（`enabled` 与运行状态分开，§7.5）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootSpec {
    pub location: RootLocation,
    pub enabled: bool,
}

/// 从 SearchSettings 投影出的索引配置（core 不读取列宽、窗口、历史等 UI 字段，§2）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexConfig {
    pub roots: Vec<RootSpec>,
    /// 用户排除规则（原文，编译交给 [`crate::scan::ExcludeSet`]）。
    pub exclude_rules: Vec<String>,
    pub exclude_case_sensitive: bool,
    /// 卷相对格式的忽略路径（与根同一模型，§7.4）。
    pub ignore_paths: Vec<RootLocation>,
    pub scan_app_bundles: bool,
    pub watch: bool,
}

/// 查询行为投影。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryOptions {
    pub case_sensitive: bool,
    /// 结果上限；0 表示不限（§7.9）。
    pub max_results: u32,
    /// 空查询是否列出全部。
    pub show_all_on_empty: bool,
}

/// 节点类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum NodeKind {
    File = 0,
    Dir = 1,
}

impl NodeKind {
    pub fn from_u8(v: u8) -> NodeKind {
        if v == 1 { NodeKind::Dir } else { NodeKind::File }
    }
}

/// 节点状态三种（§7.1 第二轮评审：磁盘删除 / 规则排除 / 批量删除中的临时隐藏，
/// 不能共用一个"墓碑"位）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum NodeStatus {
    /// 正常。
    Live = 0,
    /// 磁盘上已删除：同一 index_epoch 内 NodeId 不复用；LoadRows 不返回。
    Deleted = 1,
    /// 被排除规则掩码：可恢复；解除规则后旧 ResultId 仍指向同一文件。
    Excluded = 2,
    /// 批量删除中的临时隐藏，几毫秒后转为 `Deleted`。
    PendingRemove = 3,
}

impl NodeStatus {
    pub fn from_u8(v: u8) -> NodeStatus {
        match v {
            1 => NodeStatus::Deleted,
            2 => NodeStatus::Excluded,
            3 => NodeStatus::PendingRemove,
            _ => NodeStatus::Live,
        }
    }

    /// LoadRows / 查询是否返回该行（§7.1）。
    pub fn visible(self) -> bool {
        self == NodeStatus::Live
    }
}

/// 节点元数据（扫描时预取；Unknown 用 `None`，不用状态变化时间冒充，§7.1、§7.3）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeMeta {
    pub size: Option<u64>,
    /// 修改时间（Unix 秒）。
    pub mtime: Option<i64>,
    /// 创建时间（Unix 秒；不支持或读取失败为 `None`）。
    pub btime: Option<i64>,
}

/// 排序列（与搜索设置里的 `ui.sort` 对应；serde 用 snake_case）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortColumn {
    /// 默认路径序（rank，§7.9）。
    Path,
    Name,
    Size,
    Modified,
    Created,
}

impl SortColumn {
    /// 表头循环顺序：升 → 降 → 默认（§5.1）。
    pub const ALL: [SortColumn; 5] = [
        SortColumn::Path,
        SortColumn::Name,
        SortColumn::Size,
        SortColumn::Modified,
        SortColumn::Created,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortColumn::Path => "路径",
            SortColumn::Name => "文件名",
            SortColumn::Size => "大小",
            SortColumn::Modified => "修改时间",
            SortColumn::Created => "创建时间",
        }
    }
}

/// 排序方向；`None` 表示默认路径序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDir {
    /// 默认路径序（rank）。
    None,
    Asc,
    Desc,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_id_packs_root_and_node() {
        let id = ResultId::new(RootId(7), NodeId(0x1234_5678));
        assert_eq!(id.root(), RootId(7));
        assert_eq!(id.node(), NodeId(0x1234_5678));
        // 根 ID 只打包 u16。
        let id = ResultId::new(RootId(u16::MAX), NodeId(u32::MAX));
        assert_eq!(id.root(), RootId(u16::MAX));
        assert_eq!(id.node(), NodeId(u32::MAX));
        assert_ne!(ResultId::new(RootId(1), NodeId(2)), ResultId::new(RootId(2), NodeId(1)));
    }

    #[test]
    fn fallback_volume_identity_is_marked() {
        let id = VolumeId::fallback_absolute(Path::new("/Volumes/Data"));
        assert!(id.is_fallback());
        assert!(!VolumeId("76e8fafa-3b3c-4c97-b1d0-1f0d5a6e9e34".into()).is_fallback());
    }

    #[test]
    fn root_location_joins_mount_point() {
        let loc = RootLocation {
            volume_id: VolumeId("v".into()),
            relative_path: "Users/me".into(),
            fallback_absolute_path: PathBuf::from("/Users/me"),
        };
        assert_eq!(loc.absolute(Path::new("/")), PathBuf::from("/Users/me"));
        assert_eq!(loc.absolute(Path::new("/Volumes/Disk 1")), PathBuf::from("/Volumes/Disk 1/Users/me"));
        // 卷根：relative_path 为空。
        let root = RootLocation {
            volume_id: VolumeId("v".into()),
            relative_path: String::new(),
            fallback_absolute_path: PathBuf::from("/Volumes/Disk"),
        };
        assert_eq!(root.absolute(Path::new("/Volumes/Disk")), PathBuf::from("/Volumes/Disk"));
    }
}
