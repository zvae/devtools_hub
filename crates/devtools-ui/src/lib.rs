//! DevTools Hub 各功能模块共用的基础界面样式。
//!
//! 平台无关：字体加载与中西文基线对齐（[`fonts`]）、矢量图标（[`icons`]）、配色 / 圆角 /
//! 留白规范与连续曲率圆角绘制（[`style`]）、几个通用小控件（[`widgets`]），以及浮在内容上的
//! 栏的玻璃背景（[`glass`] 在 egui 里模拟、[`liquid_glass`] 用 wgpu 着色器做真正的磨砂玻璃）。
//!
//! 与业务耦合的部分（剪贴板内容类型的代表色、时间 / 数字格式化、依赖窗口平台层的拖动区域与
//! 窗口圆角取值等）不放在这里，由宿主传入或自行定义。

pub mod fonts;
pub mod glass;
pub mod icons;
pub mod liquid_glass;
pub mod style;
pub mod widgets;
