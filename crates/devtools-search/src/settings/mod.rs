//! 搜索设置：模型与存储（§6）。

mod model;
mod store;

pub use model::{
    ColumnWidths, HistoryEntry, ResultView, RootConfig, SavedSearch, SearchSettings,
    SearchWindowMode, UiPrefs, UiSort, WindowPrefs, SETTINGS_VERSION,
};
pub use store::{LoadOutcome, SearchSettingsStore};

// 设置的公开数据模型里用到的 core 纯类型，一起导出，宿主构造索引根时不必单独依赖 core。
pub use devtools_search_core::{FileKind, RootLocation, VolumeId};
