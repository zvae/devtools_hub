//! devtools-search-core：文件搜索模块的纯核心。
//!
//! 不依赖 egui、平台 API 或外层 crate（§3.3 由依赖图保证：`cargo check -p devtools-search-core`
//! 必须独立通过）。核心认识的数据只有纯类型与 ports：
//! - [`model`]：ID、根位置、索引/查询配置等纯数据类型；
//! - [`text`]：NFC 归一化与不区分大小写文本规则（§7.1）；
//! - [`index`]：SoA 节点表、共享名字 arena、节点三种状态、epoch/revision（§7.1）；
//! - [`query`]：语法解析与求值、排序与 top-k（§7.8、§7.9）；
//! - [`scan`]：目录遍历、排除规则剪枝、取消（§7.3、§7.4）；
//! - [`snapshot`]：每根快照格式、校验、manifest（§7.6）；
//! - [`volumes`]：根状态机与 root_key 身份规则（§7.5）；
//! - [`ports`]：平台能力的 trait 定义（FsSource / Watcher / Volume / Icon / SystemActions /
//!   FolderPicker / SystemModalGuard / AtomicStorage）。

pub mod index;
pub mod model;
pub mod ports;
pub mod query;
pub mod scan;
pub mod snapshot;
pub mod text;
pub mod volumes;

pub use index::RootIndex;
pub use model::{
    IndexConfig, NameId, NodeId, NodeKind, NodeMeta, NodeStatus, QueryOptions, ResultId, RootId,
    RootLocation, RootSpec, SortColumn, SortDir, VolumeId,
};
pub use ports::{Entry, EntryKind, FsSource, VolumeSource};
pub use volumes::{root_key, RootState};
