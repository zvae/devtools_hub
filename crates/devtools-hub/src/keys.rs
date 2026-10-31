//! 按键在界面上的写法：macOS 用 ⌘ ⇧ ↩ 等符号，Windows 用 Ctrl、Shift、Enter 等文字。
//!
//! 实现在共用模块 `devtools_ui::keys`（文件搜索页也要用同一套写法），这里只做转发，
//! 让 `crate::keys::*` 的调用路径保持不变。

pub use devtools_ui::keys::*;
