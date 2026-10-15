//! DevTools Hub 剪贴板历史核心库：与平台无关的数据模型、内容分析、过滤规则和存储。

pub mod codec;
pub mod model;
pub mod rules;
pub mod store;
pub mod thumbnail;
pub mod uti;

pub use model::{Capture, ClipItem, ClipList, ContentKind, Representation, Snapshot, SourceApp};
pub use rules::{CaptureRules, ExcludedApp, SkipReason};
pub use store::{ItemQuery, KindFilter, RecordOutcome, Store, StoreStats, FAVORITES_LIST_ID};
