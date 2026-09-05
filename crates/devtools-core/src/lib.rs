// core 层只放应用请求/事件调度和搜索模型，避免依赖具体 UI 实现。
pub mod runtime;
pub mod search;

// 对外导出核心类型，让 app 层可以用统一入口连接服务、存储和 UI。
pub use runtime::{AppEvent, AppRequest, AppRuntime, TimestampConversionMode};
pub use search::{SearchEngine, SearchItem, SearchResult};
