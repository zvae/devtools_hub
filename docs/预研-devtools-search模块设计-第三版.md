# devtools-search 文件搜索模块设计方案

> 状态：第三版。按 2026-10-30 第二轮评审修订；用于 M0 验证与后续实施，不表示功能已经实现。  
> 修订日期：2026-10-30  
> 前序版本：第一版（2026-10-29）、第二版（2026-10-30）。第二轮评审输入：《预研-devtools-search模块设计-第二版评审意见.md》。  
> 代码基线：[zvae/devtools_hub，679ab298f4b7d564616015c6daf435a17e40003e](https://github.com/zvae/devtools_hub/tree/679ab298f4b7d564616015c6daf435a17e40003e)。  
> 本次范围：修改方案文档。下文目录、接口和配置均为拟实施设计；性能数字是待验证目标。

## 本版相对第二版的变化

1. **补上退出生命周期**：托盘退出、更新重启不再 `std::process::exit`，改为正常关闭；`HubApp` 增加 `on_exit`，搜索模块区分"非阻塞禁用"和"有界阻塞退出"两套关闭语义。
2. **原生对话框与窗口拖动归宿主**：`FolderPicker`、系统模态保护、顶栏拖动仍由模块定义 trait，但实现放到宿主平台层——它们必须操作宿主窗口的层级、应用激活和焦点恢复。这是"模块自包含"唯一的机制让渡，在 §0、§3、§8 写明。
3. **去掉 `suppress_hide_on_blur`，拆开 `poll()`/`ui()` 的返回值**：阻塞式系统对话框期间 `ui()`/`poll()` 不运行，跨帧传布尔值来不及；改由宿主的 `dialog_open()` / `just_closed_dialog` 统一处理，`poll()` 只返回策略状态。
4. **行批次与 revision 解耦**：行内容只校验会话/查询代际/根/epoch + 逐行状态；`revision` 只让结果集与排序失效，避免持续写入时结果区反复空白。
5. **墓碑拆成三种状态**：`deleted` / `excluded` / `pending_remove`，分别对应磁盘删除、规则排除、批量删除中的临时隐藏。
6. **搜索页窗口行为写实**：沿用 101 层级与共享窗口，但进入搜索页时切到 `MoveToActiveSpace`、宿主把页面底画成不透明；Pinned 不再跟随所有桌面和全屏应用。
7. **第一轮+第二轮评审的其余意见**：默认排除 `~/Library`、配置从 version 1 开始、`RootLocation` 归 core、托盘入口改禁用态、快捷面板 FileSearch 语义、高亮映射与名字预算、缓存清理、M0 拆分等，逐项落到正文与附录 B。

## 0. 方案摘要与确定边界

1. **搜索业务全部归属 `crates/devtools-search/`**：搜索界面、搜索设置及持久化、索引、查询、扫描、监听、快照、图标、权限引导逻辑、平台适配都在该目录内。
2. **三项系统机制由宿主提供**：原生文件夹选择（`NSOpenPanel` / `IFileDialog`）、系统模态期保护、无边框窗口的顶栏拖动。它们必须操作宿主窗口的层级、应用激活、焦点恢复和拖动，模块拿不到窗口；trait 定义在模块 `ports`，实现放 `devtools-hub` 的平台层。模块只决定"何时选、初始目录、结果如何校验和应用"。
3. **主模块只负责接入**：页面/托盘/快捷面板入口、全局搜索快捷键的配置与注册、功能开关、正常退出与更新重启的生命周期。主设置只新增 `search_enabled: bool = false`、`search_hotkey: String` 两个字段。
4. **默认关闭**：关闭时不创建 `SearchModule`，不读取搜索设置或快照，不启动搜索线程、设备监听或目录扫描，不注册搜索快捷键；托盘菜单项保留但禁用。所有入口受同一个开关控制，过期事件不能绕过开关。
5. **搜索设置独立**：齿轮入口、表单、默认值、校验、迁移、读写全部由搜索模块完成，不并入 Hub 设置页；主模块不知道根目录、排除规则、列宽、历史。
6. **模块内严格分层**：`ui → application → core/ports`，`platform` 实现 ports，`settings` 提供配置模型和存储；纯核心以模块目录内的嵌套 crate 提供编译隔离。`RootLocation`、`VolumeId` 等纯数据类型归 core，`settings` 单向依赖 core 的纯类型。
7. **v1 只做 `Floating` / `Pinned`**；`Normal`、独立视口、Dock/任务栏入口留到 v2。搜索页沿用共享窗口，但进入时切到"活动桌面"的集合行为、由宿主画不透明底（§5.8），不再是"跟随所有桌面的大玻璃面板"。
8. **默认根为主目录，默认懒加载**；外置卷必须显式添加。macOS 首次扫描前有权限引导；Windows 加载旧快照后后台校验，并支持安全弹出外置盘。
9. **正常退出要尽力保存**：关闭窗口/退出应用走 eframe 的 `on_exit`，有界等待快照与设置落盘；崩溃、强杀不保证保存，恢复依赖检查点和校验。
10. **M0 同时验证 macOS / Windows**，先固定并发、数据结构、权限、监听、对话框和移除设备的契约，再进入完整实现。

## 1. 目标与非目标

### 1.1 目标

- 提供顶部搜索栏、文件夹范围、五列结果表格、状态栏，以及模块内搜索设置。
- 支持文件名/路径、布尔表达式、通配符、大小/日期等元数据过滤；搜索与排序在后台执行。
- 支持多根、目录排除、实时更新、按根快照、离线卷、外置盘插拔；只读取文件系统元数据。
- 不拖慢默认关闭状态下的 Hub 启动；启用但保持懒加载时，首次打开前不建立索引。
- 不改变剪贴板、JSON、OCR 等页面自身的业务行为；共用窗口的页面切换、失焦、最小尺寸、退出由宿主统一处理。
- 搜索页在 Pinned 下是"置顶但只存在于当前桌面"的普通浮层：切桌面不跟随，热键把它拉到当前桌面；表格用不透明底保证可读性。

### 1.2 非目标

- 文件内容搜索、trigram 内容索引、Spotlight 集成、HTTP/MCP/CLI daemon。
- Quick Look、缩略图、拖出、重命名、移到废纸篓、打开终端。
- 自动扫描所有磁盘；v1 不支持网络卷索引，添加网络根时明确提示不支持。
- 拼音/拼音首字母搜索列入 v2。
- 普通窗口、多窗口并存、Dock/任务栏入口列入 v2。
- Windows USN Journal 列为 v2 调研项。
- 完整的无障碍支持不在 v1；v1 只保证自绘表格有可读的行/列标签（§5.7）。

## 2. 设计原则

1. **单向依赖**：核心不认识 UI、宿主、搜索设置全量结构或平台实现；只接收纯数据和 ports。
2. **配置单一写入方**：`application::SearchService` 持有唯一权威 `SearchSettings`；UI 只持有表单草稿和只读视图，通过命令提交修改。
3. **核心配置投影**：Service 从 SearchSettings 派生 `IndexConfig` / `QueryOptions`，核心不读取列宽、窗口、历史等 UI 字段。
4. **后台完成重工作**：UI 不遍历文件系统、不持索引锁、不等待线程退出或快照写盘。
5. **保留真实文件身份**：归一化文本仅用于匹配；系统操作使用原始路径，不使用显示字符串或归一化后的文件名重建操作路径。
6. **默认关闭优先级最高**：主功能开关覆盖模块内部的懒加载、监听和隐藏后继续索引设置。
7. **失败可观察**：权限拒绝、离线、校验中、更新丢失、快照错误、对话框失败都呈现状态，不能把过期数据标成"就绪"。
8. **系统机制可让渡**：只有必须依附宿主窗口的能力（原生对话框、应用激活、窗口层级、退出生命周期）交给宿主；搜索业务、索引和设置不越界。
9. **结果可以陈旧，但身份不能错**：允许显示旧的大小/时间或稍旧的排序，绝不允许把旧 ResultId 显示成另一个文件。
10. **正常退出有界保存**：退出允许有限阻塞等待落盘；运行中的禁用不允许阻塞 UI。

## 3. 总体架构

### 3.1 依赖与目录

```text
devtools-hub
  └─ devtools-search（唯一集成 API）
       ├─ ui ──────────→ application 的命令/只读视图
       ├─ application ─→ settings + devtools-search-core
       ├─ platform ───→ devtools-search-core::ports
       └─ settings ───→ core 的纯数据类型（RootLocation / VolumeId / IndexConfig）

宿主平台层（devtools-hub/src/platform）
  ├─ 实现 core::ports::FolderPicker（NSOpenPanel / IFileDialog）
  ├─ 实现 core::ports::SystemModalGuard（激活、降级层级、焦点恢复）
  └─ 只做机制，不做搜索策略

crates/devtools-search/
├─ Cargo.toml                  # Hub 只依赖这个 crate
├─ src/
│  ├─ lib.rs                   # 公开 API、组合根，接收 HostServices
│  ├─ application/
│  │  ├─ mod.rs
│  │  ├─ service.rs            # 命令、状态、生命周期、唯一设置所有者
│  │  ├─ protocol.rs           # Command/Event/ViewModel
│  │  ├─ config.rs             # SearchSettings → IndexConfig
│  │  └─ runtime.rs            # 专用线程池、任务取消、平台事件编排
│  ├─ settings/
│  │  ├─ model.rs              # SearchSettings、默认值、版本迁移
│  │  └─ store.rs              # JSON 编解码、校验、原子存储编排
│  ├─ ui/
│  │  ├─ mod.rs
│  │  ├─ header.rs
│  │  ├─ results_table.rs      # 虚拟化、选择、列宽
│  │  ├─ status_bar.rs
│  │  ├─ settings_panel.rs     # 搜索设置，不进入主设置页
│  │  ├─ permissions.rs        # 首次扫描引导、拒绝目录列表
│  │  ├─ history.rs
│  │  ├─ icons.rs             # 仅纹理上传、缓存
│  │  ├─ text.rs
│  │  └─ theme.rs
│  └─ platform/
│     ├─ mod.rs               # 平台实现选择
│     ├─ macos/               # fs/watch/volumes/icons/actions/storage
│     └─ windows/             # 同上，另含 device_events 消息线程
├─ core/
│  ├─ Cargo.toml              # package.name = devtools-search-core
│  ├─ src/
│  │  ├─ lib.rs
│  │  ├─ ports.rs             # FsSource / Watcher / Volume / Icon / Action / FolderPicker / SystemModalGuard
│  │  ├─ model.rs             # ID、元数据、IndexConfig、RootLocation、VolumeId
│  │  ├─ index.rs             # SoA、名字缓冲区、墓碑/排除状态、版本
│  │  ├─ query/               # syntax/parse/eval/sort
│  │  ├─ scan/                # 遍历调度、取消、exclude
│  │  ├─ snapshot.rs          # 文件格式、校验、检查点
│  │  └─ volumes.rs           # 根状态和身份规则
│  └─ tests/                  # 查询、排除、快照、ID、并发一致性
└─ tests/
   ├─ lifecycle.rs
   ├─ settings.rs
   ├─ ui.rs
   └─ layering.rs
```

根 workspace 增加 `crates/devtools-search` 与 `crates/devtools-search/core` 两个成员；外层 crate 用 path 依赖内部核心。两者源代码、测试、文档和平台实现均在 `devtools-search/` 子树下。核心不依赖外层 crate，Hub 不直接依赖核心。

`FolderPicker` / `SystemModalGuard` 的**实现**在 Hub 的 `platform/`，但 trait 和用例在模块内。这样既不把 `NSWindow`/`HWND` 暴露成公开类型，也不让模块假装自己拥有窗口。

### 3.2 分层职责

| 层 | 职责 | 禁止 |
| --- | --- | --- |
| lib.rs | 装配、公开 facade、转发生命周期、注入 HostServices | 查询求值、配置规则、平台实现 |
| ui | 绘制、输入、草稿、只读视图、可见行请求 | 读写配置文件、访问索引内部、直接调用平台 API |
| application | 配置唯一来源、用例编排、状态机、异步任务 | 表格绘制、原生系统调用细节 |
| core | 索引、查询、排除、快照格式、根状态、纯数据类型 | egui、objc2、windows-sys、外层 settings/UI、文件对话框实现 |
| platform（模块内） | 文件系统、监听、设备、图标、系统动作、原子文件替换 | 业务规则、UI 文案、读取 SearchSettings、原生对话框 |
| settings | 配置默认值、迁移、校验、JSON 存储编排 | UI、扫描、索引逻辑 |
| core::ports | traits、纯数据、错误类型 | 反向依赖、转导出 GUI/平台类型 |
| 宿主平台能力 | FolderPicker 实现、模态期保护、窗口拖动、退出钩子 | 搜索策略、根/排除/查询配置 |

平台相关 `cfg` 和系统 API 集中在 `platform/`；核心 `#[cfg(test)]` 等测试条件不受此约束。UI 经 application 发起图标、打开文件和文件夹选择用例，不直接调用 platform。

### 3.3 分层检查

- 首先由核心 crate 的 Cargo 依赖图保证没有 GUI/平台/外层业务依赖，CI 单独运行 `cargo check -p devtools-search-core`。
- 核心仅导出业务 API 和 DTO；索引内部字段和子模块保持私有，不能用外层 `pub(crate)` 充当 UI 隔离。
- `layering.rs` 检查外层 ui/application/platform/settings 边界及核心依赖白名单；用 `syn` 解析 Rust AST，检查绝对 use、别名、重导出和 `super` 路径，不再用去注释后正则匹配。
- 明确允许 `settings → core::model` 的单向依赖；禁止 `core → settings`。`RootLocation` 等类型只在 core 定义一份。
- 对宏展开、`include!` 等不宣称 AST 检查完全可靠；禁止用它们跨层导入，结合依赖图与代码评审检查。新增依赖必须更新白名单及理由。
- 宿主能力实现不在本 crate 内，无法用 `layering.rs` 检查；通过 trait 签名和代码评审约束：宿主实现不得读取 `SearchSettings`，模块不得把 `NSWindow`/`HWND` 放进公开类型。
- 与仓库现有 CI 一致，在 Windows 和 macOS 跑 workspace check/test；不依赖机器安装 `rg`。

### 3.4 依赖决策

| 依赖 | 用途 / 所属 |
| --- | --- |
| egui（跟随 workspace 0.36） | 外层 UI；核心不依赖 |
| serde / serde_json、anyhow、tracing | 配置、错误、日志；沿用 workspace |
| directories | 模块解析本地缓存位置；沿用 workspace |
| zstd / sha2 | 每根快照压缩、校验、稳定根键；沿用 workspace |
| globset / regex / postcard | 排除、查询正则、紧凑序列化 |
| rayon | 模块专用查询池和扫描池；禁止全局池 |
| unicode-normalization（必选） | 匹配文本 NFC 归一化 |
| objc2 系列 / libc / FSEvents 相关绑定 | macOS 模块平台：文件、监听、卷、图标、系统动作 |
| windows-sys，必要时 windows COM 绑定 | Windows 模块平台：文件、监听、卷、图标、系统动作 |
| tempfile、egui_kittest、syn | 开发依赖：临时目录、UI 测试、AST 分层检查；egui_kittest 与 egui 版本对齐 |

原生对话框不再由模块实现，模块平台不新增 `NSOpenPanel` / `IFileDialog` 依赖；宿主本来就有 AppKit / `GetOpenFileNameW` 所需的能力。不引入 thin-vec（改用紧凑列/平面缓冲区）、HTTP/MCP/内容搜索库、rfd。

表格暂保留自定义虚拟化，以复用现有交互风格；不声称 egui_extras 不支持虚拟化或拖动列宽。M0 对比 TableBuilder，记录自定义表格预计 2–3 人天成本；如需改选，先更新本节依赖和工时结论。

## 4. 对外 API 与主模块接入

### 4.1 公开 API 契约

以下为接口草案，辅助类型需在实现时完整定义；不作为可直接编译的示例。

```rust
pub struct SearchModule { /* 组合根持有 UI + ServiceHandle */ }

pub struct SearchOptions {
    pub query: Option<String>,
    /// 入口带来的选中路径：单个目录作为范围；单个文件取父目录；
    /// 多路径仅在能确定共同目录时设置范围，否则保留原范围并提示。
    pub selected_paths: Vec<PathBuf>,
    /// true 表示 query 是入口带来的原始文本，按字面短语搜索，不解析布尔/过滤器语法。
    pub query_is_literal: bool,
}

pub enum SearchWindowMode { Floating, Pinned }

pub struct WindowPolicy {
    pub preferred_size: [f32; 2],
    pub min_size: [f32; 2],
    pub mode: SearchWindowMode,
    pub hide_on_blur: bool,
}

pub struct SearchUiOutput {
    /// 用户按 Esc / 关闭键，请求宿主隐藏窗口。只有 ui() 产生。
    pub request_hide: bool,
    pub policy_changed: bool,
}

pub struct SearchPoll {
    /// 非 UI 路径引起的策略变化（设置应用、屏幕变化等）。
    pub policy_changed: bool,
}

/// 宿主窗口能力：只做机制，不做搜索策略。
pub trait HostServices: Send + Sync {
    /// 在主线程执行原生文件夹选择。实现负责应用激活、浮层降级、
    /// 失焦抑制、焦点恢复；取消返回 Ok(None)，失败返回错误。
    fn pick_folder(&self, initial: Option<&Path>) -> Result<Option<PathBuf>, HostError>;

    /// 进入系统模态期（首次权限引导、系统授权提示）。
    /// guard 存活期间宿主视同 dialog_open()，不因失焦收起搜索页。
    fn begin_system_modal(&self) -> Box<dyn SystemModalGuard>;

    /// 在搜索页顶栏空白处开始拖动窗口。宿主转发现有的 panel::start_drag / winit，
    /// 返回 false 表示宿主未处理，模块回退到 ViewportCommand::StartDrag。
    fn start_window_drag(&self) -> bool;
}

pub trait SystemModalGuard: Send {
    /// 显式结束模态期并恢复窗口层级；restore_focus=false 时把焦点留给搜索页。
    /// 实现也必须在 Drop 中恢复，避免早退/错误路径漏掉。
    fn end(self: Box<Self>, restore_focus: bool);
}
// HostServices 是 facade，内部组合 core::ports 的 FolderPicker / SystemModalGuard；
// application 只依赖 facade，不直接认识宿主平台类型。

impl SearchModule {
    /// 只在主开关开启后调用。host 由宿主实现并注入。
    /// 启动轻量配置加载；不启动索引/卷枚举/watch，不同步等待大文件 I/O。
    pub fn new(data_dir: &Path, ctx: egui::Context, host: Arc<dyn HostServices>) -> Self;

    pub fn warm_up(&mut self);
    pub fn poll(&mut self) -> SearchPoll;
    pub fn ui(&mut self, ui: &mut egui::Ui) -> SearchUiOutput;
    pub fn on_shown(&mut self, options: SearchOptions);
    pub fn on_hidden(&mut self);
    pub fn window_policy(&self) -> WindowPolicy;
    pub fn remember_size(&mut self, size: [f32; 2]);

    /// 运行中禁用：拒绝新任务、取消工作、释放 watcher/扫描句柄、按配置尝试保存；不阻塞 UI。
    pub fn shutdown(&mut self);
    /// 正常退出：在 shutdown 的基础上有界阻塞，返回是否在期限内完成。
    pub fn shutdown_blocking(&mut self, timeout: Duration) -> bool;
    pub fn is_stopped(&self) -> bool;
}
```

- `new` 只在主开关开启后调用，启动轻量配置加载；不做卷枚举、不建 watcher、不扫盘。
- 宿主启用后调用一次 `warm_up()`；Service 等待配置加载完成，仅当 `lazy_start=false`、未暂停且权限引导已完成才后台启动。否则保持 Dormant / AwaitingConsent。
- `on_shown()` 聚焦输入框并请求初始化；首次使用先显示权限/范围引导，确认后扫描。可重复调用，不重复建线程池。
- `poll()` 在宿主 `logic` 阶段执行，即使搜索页隐藏也能处理结束事件、配置结果、平台主线程任务及关闭完成状态。只返回策略变化；`request_hide` 只由 `ui()` 产生。宿主对两处的 `policy_changed` 取或即可，避免"隐藏时收到 hidden 页的 hide 请求"。
- `on_hidden()` 表示离开可见搜索页；是否后台继续运行由模块设置决定，不等同于禁用功能。
- `remember_size()` 只提交尺寸给 Service；隐藏、切页、禁用、退出前均提交一次。尺寸限制到当前显示器可用区域，小屏幕不强行撑到 800×480。
- `shutdown()` 与 `shutdown_blocking()` 的语义分开：前者用于运行中禁用，后者只用于正常退出。`is_stopped()` 只用于"禁用→立即重新开启"的竞态轮询，退出时事件循环已经结束，轮询不到。
- 崩溃、强杀、断电不保证保存；旧快照保持有效，下次启动按检查点/校验恢复，不能承诺"退出必落盘"。

### 4.2 主模块最小接入清单

| 文件/区域 | 允许的改动 |
| --- | --- |
| 根 Cargo.toml、Cargo.lock | 两个模块内部成员、外层 path 依赖及实际所需依赖锁定 |
| devtools-hub/Cargo.toml | 只新增对 devtools-search 外层 crate 的依赖 |
| src/settings.rs | 新增 `search_enabled=false`、`search_hotkey`，补默认值和 hotkey 访问分支；旧配置缺字段时保持关闭 |
| src/hotkey.rs | `HotkeyAction::Search`、名称/索引/`ALL`（长度 4）、事件分派；关闭时 suspend，不因录制结束重新注册 |
| src/shared.rs | 搜索入口事件、退出/关闭事件；不存搜索配置或索引 |
| src/tray.rs | 文件搜索菜单项常在，关闭时 `set_enabled(false)` 并显示"（未启用）"；退出改发事件而不是 `process::exit` |
| src/tools/mod.rs、src/ui/quick_panel.rs | 按开关提供 FileSearch；调整 `actions_for` / `has_match`；选中文本/路径转成 SearchOptions（文本走 `query_is_literal`） |
| src/ui/settings_page/general.rs 及原有设置应用路径 | 仅"启用文件搜索"开关和快捷键行；开关关闭时说明快捷键不生效 |
| src/ui/mod.rs（可拆通用导航文件） | `Option<SearchModule>`、`Page::Search`、统一 `set_page`、窗口策略、poll、`on_exit`、draw 的 inner_margin/toast（Search 用 0 边距、toast 用现有页面的底部偏移）、搜索页不透明底色分支、handle_focus 分支；`hotkey_errors` 数组改为 4 项 |
| src/controller.rs 及直接 show/hide 的入口调用点 | 一律 post 事件，由 `HubApp::logic` 处理；不在 controller 里直接 `panel::hide` |
| 模块内 `ui/mod.rs` 的 `self.page = ...`（6 处：329/357/393/400/492/522）与 `panel/mod.rs:437`、`settings_page/mod.rs:214`、`ocr.rs:253` | 统一走 `set_page(from, to)`，离开搜索页时完成尺寸保存、失焦策略恢复、最小尺寸恢复 |
| Hub 原生 panel 适配 | macOS 增加 `set_search_space_behavior(bool)`：进入搜索页切 `MoveToActiveSpace`，离开恢复 `CanJoinAllSpaces` / `FullScreenAuxiliary` / `IgnoresCycle`；Windows no-op |
| src/platform/{macos,windows}/ | 实现 `FolderPicker`、`SystemModalGuard` 与 `start_window_drag`（转发现 `panel::start_drag` / winit）；复用 `save_dialog` 已有的激活/降级/恢复逻辑；不实现搜索业务 |
| src/ui/mod.rs（on_exit）、main.rs、update.rs | 正常退出：托盘退出/更新重启改为 `ViewportCommand::Close` 或事件；`on_exit` 做 remember_size + shutdown_blocking；更新重启在 `run_native` 返回后重新拉起 |
| docs/ | 更新窗口生命周期说明，避免下次再直接 `process::exit` |

清单以基线代码为依据：当前 `controller.rs` 可直接隐藏窗口，`ui/mod.rs` 多处直接赋值 page，必须逐一收口；不能只改 `show_search/hide_panel`。当前 `eframe::App` 实现没有退出钩子，托盘与更新重启都直接 `process::exit`，必须一并改。另需处理 `ui/snapshot_tests.rs` 里对 `Page` 的匹配和离屏渲染用例。

Hub 不新增搜索业务控制器，不持有 `RootId`、`IndexConfig`、排除规则、列宽、历史或快照路径。主模块接线不以"150–200 行"作为硬承诺；以改动职责和依赖审查验收。

### 4.3 开关、设置和缓存边界

| 内容 | 保存位置 / 所有者 |
| --- | --- |
| search_enabled / search_hotkey | `<data_dir>/settings.json`，Hub |
| 搜索所有业务和 UI 配置 | `<data_dir>/search/settings.json`，搜索 Service |
| 可再生成快照与临时文件 | 系统本地缓存目录下的 `DevToolsHub/search/<instance_key>/index/`，搜索模块 |

macOS 缓存基目录为 `~/Library/Caches/DevToolsHub`；Windows 为 `%LOCALAPPDATA%/DevToolsHub/Cache`。`instance_key` 取规范化 data_dir 的稳定哈希，隔离 `--data-dir` / 环境变量指定的多个实例；缓存目录解析由模块完成。

仓库当前 `resolve_data_dir()` 已使用 `data_local_dir()`，Windows 本来就在 LOCALAPPDATA，不虚构迁移。保持现有设置位置，仅明确搜索快照的缓存属性和独立位置。依据：[基线 settings.rs](https://github.com/zvae/devtools_hub/blob/679ab298f4b7d564616015c6daf435a17e40003e/crates/devtools-hub/src/settings.rs)。

缓存按实例隔离，但换 `--data-dir` 后旧实例目录会残留：Service 在启动和退出时清理本应用缓存根下超过 30 天未使用的实例目录，只删自己确认的路径，不碰用户文件。

不存在第二套快捷键归属方案；固定由 Hub 管理。模块内不再设置功能总开关，仅有"暂停/恢复索引"等运行控制，防止两套 enabled 冲突。

### 4.4 生命周期与导航

| 场景 | 行为 |
| --- | --- |
| 应用启动，功能关闭 | search=None；不读搜索目录、不注册搜索热键；托盘项禁用 |
| 开启功能 | 创建模块，注入 HostServices，调用 warm_up，注册热键并启用入口；默认 lazy_start=true，不扫描 |
| 首次打开 | 模块读取配置完成后显示引导；确认范围/权限策略后加载快照、启动对应根 |
| 隐藏或切到其他页 | 保存搜索尺寸、on_hidden、释放未完成的系统模态 guard、恢复目标页尺寸/最小值/失焦策略/桌面集合行为 |
| 热键，搜索页当前有焦点 | 隐藏并走统一离开路径 |
| 热键，搜索页可见但无焦点 | 拉到当前桌面并聚焦，不能误收起 |
| 其他页面/隐藏时触发入口 | 检查开关，进入搜索页并应用 WindowPolicy |
| 运行中关闭功能 | 立即屏蔽入口和注册，离开搜索页，shutdown；`logic` 轮询 `is_stopped()`，完成后释放实例；期间开关显示"正在停止" |
| 关闭后立即重新开启 | 等待旧实例完全停止再创建新实例；旧会话事件丢弃，避免双 watcher |
| 原生对话框打开 | 宿主执行并持有模态保护；UI 线程被原生的 `runModal` / `GetOpenFileNameW` 阻塞，热键和禁用请求在对话框关闭后处理；不强行关闭系统对话框 |
| 应用退出 | 托盘/菜单退出改发关闭事件；`on_exit`：remember_size → shutdown_blocking(≤2s) → 返回；更新重启在 `run_native` 返回后重新拉起 |
| 崩溃/强杀 | 不保证保存；下次加载按检查点回放或校验，旧快照保持有效 |

主模块提供通用 `set_page(from, to)` / `hide_current_page()` 收口：离开回调 → 保存尺寸 → 恢复通用窗口策略 → 进入目标页。托盘、热键、设置页、快捷面板、JSON/OCR、隐藏及退出均不能绕过。`controller::toggle_panel` / `toggle_from_status_item` 不再直接 `panel::hide`，一律 post 事件。

v1 两种搜索模式均置顶且沿用现有浮层窗口。进入搜索页时：设置最小尺寸、切到 `MoveToActiveSpace`、宿主把页面底画成不透明；离开时恢复 `CanJoinAllSpaces`、最小尺寸和通用失焦策略。Pinned 只影响失焦策略，不引入 Normal 模式。宿主收到 `policy_changed` 后更新最小尺寸和行为，不在每帧重新覆盖用户拖动后的窗口尺寸。

失焦判断必须先消费 `poll()` 状态，并尊重宿主的 `dialog_open()` / `just_closed_dialog`。搜索模块不再提供 `suppress_hide_on_blur`：宿主持有原生对话框的会话，天然知道何时暂停失焦隐藏，比跨帧传布尔值可靠。

### 4.5 快捷键与传参

- 默认 macOS `Cmd+Shift+Space`，Windows `Alt+Shift+F`；允许修改或清空。注册冲突在主快捷键设置行提示，不影响托盘入口。
- Windows 中文输入法需实测 Alt+Shift 输入语言切换冲突；M0 记录结果，必要时修改默认值，不能默认为无冲突。
- 选中文本作为查询时走 `query_is_literal=true`：当作字面短语，不解析 `|`、`!`、`(` 等语法；超过 512 字符或含换行时截到首行/前 512 字符并提示，避免把整段代码当查询。
- 选中文件路径原样交给模块：单个目录作为范围，单个文件取父目录；多路径仅在可确定共同目录时设置范围，否则保留原范围并提示。
- 文件夹范围仅过滤已配置索引，不能因入口传参自动增加根、扫描未授权目录。

## 5. 界面与交互

### 5.1 布局与表格

```text
┌──────────────────────────────────────────────────────────────────┐
│ [Aa] [历史] [ 搜索文件或 ext:pdf ...       ] [文件夹×] [搜索设置⚙] │
│ 权限、校验、离线或错误提示（按状态显示）                            │
├──────────────────────────────────────────────────────────────────┤
│ 文件名             路径                 大小    修改时间  创建时间 │
│ [icon] report.md   ~/work/docs          4.2KB   …         …        │
│ [icon] src         ~/work/project              …         …        │
├──────────────────────────────────────────────────────────────────┤
│ ● 就绪 · 1,234,567 项 · 38 个结果 · 6ms                 [固定]     │
└──────────────────────────────────────────────────────────────────┘
```

- 首选 1100×700、最小 800×480 逻辑像素，受屏幕可用区域约束。默认 Floating；固定按钮在 Floating/Pinned 间切换并发出 policy_changed。
- 五列默认权重参考 260/450/100/155/155，按内容区分配；列宽拖动、排序、窗口尺寸存到搜索设置。
- 行高约 24、表头 26、状态栏 32；表头固定，可见行加 overscan 虚拟化，服务每次最多返回 128 行。只在 UI 上传纹理。
- 文件名前 16px 文件图标；路径列显示父目录；大小、日期右对齐并使用等宽数字（`FontId::monospace` 或等宽特性），避免表格数字跳动。
- 搜索页的背景由宿主在 `draw()` 的 `Page::Search` 分支用 `ui.visuals().window_fill` 画成不透明、同圆角的底（沿用现有 `style::continuous_rect`），不跟随玻璃/半透明材质；大表格压在桌面上不可读。模块只画内容，其他页面保持现状。
- 无标题栏：只在顶栏空白区提供拖动，调用 `HostServices::start_window_drag()`（宿主转现有 `panel::start_drag` / winit）；不对整页 `max_rect` 拖拽，避免抢表格手势。
- 点击表头循环升序/降序/默认路径序；单击、Shift 连选、Cmd/Ctrl 点选、上下键移动、结果区全选；从第一行向上回到搜索框。
- 双击打开；菜单提供打开、在 Finder/资源管理器中显示、复制路径/名字。
- 表格的行/列提供可读的无障碍标签；v1 不承诺完整读屏体验。

### 5.2 键盘、历史和 IME

| 操作 | 行为 |
| --- | --- |
| Cmd/Ctrl+F | 聚焦搜索框 |
| Enter，搜索框焦点 | 立即提交搜索，记录一条有效非空历史 |
| Enter，结果区焦点 | 打开选中项，并记录当前有效查询 |
| Enter，设置输入/按钮焦点 | 由当前控件处理，不能打开结果 |
| Cmd/Ctrl+O / R / Shift+C | 结果操作：打开 / 显示 / 复制路径 |
| Cmd/Ctrl+A | 搜索框选全文，结果区才全选结果 |
| Esc | 先交给 IME/模态层；普通搜索态非空时清空，空时请求隐藏 |

输入防抖默认 100ms。IME Preedit 期间不发查询；Commit 后合并成一次查询。空 Preedit、连续 Commit、组合期 Enter/Esc 均纳入状态机测试，具体 egui 事件形状以当前版本编译验证，不能访问 TextEdit 私有组合状态。

历史只在提交或打开结果时记录，去重并保留最近 100 条；不按字符记录，失败解析不入库。支持"记录历史"开关和"清除历史"；明确历史以明文保存在本机。保存搜索记录查询、范围和 Aa，单独管理。

### 5.3 首次扫描与权限

首次打开先确认默认主目录范围和内置排除项。macOS 用户可选择：

1. 先打开系统设置配置完全磁盘访问，返回后重新检查可访问性并继续；必要时提示重新启动。
2. 按现有权限继续，说明访问 Desktop/Documents/Downloads、云目录或外置卷时可能出现系统授权提示。

不保证系统"不弹窗"，也不保证提示次数；实际行为取决于系统版本、签名及历史授权。权限检测不靠预扫描受保护目录诱发提示；没有一个全局布尔值能够证明所有目录可读。扫描记录拒绝/不可访问目录数，可展开查看并重试；拒绝后其他根继续。

后台预热尚未完成引导时停在 AwaitingConsent，不能在用户未打开搜索页时触发首轮受保护目录扫描。首次授权/引导阶段（系统弹窗可能出现的窗口）通过 `HostServices::begin_system_modal()` 进入系统模态期：宿主临时降级浮在普通层级之上的窗口、暂停失焦隐藏，结束后恢复；这样系统授权框不会被 101 层级的面板盖住，也不会因为焦点变化把搜索页收起。引导结束、开始后台扫描后释放 guard。TCC 弹窗的实际层级和行为在 M0 实测。

其他空态：未初始化、扫描中、正在校验旧快照、无结果、超过结果上限、离线、暂停、更新可能滞后、快照损坏重建。任何旧快照校验期间均清楚显示"结果可能尚未更新"。

### 5.4 文件夹选择与拖入

- 模块的 `application` 发起 `pick_folder` 用例，调用注入的 `HostServices::pick_folder`；宿主打开原生对话框并负责应用激活、窗口层级、失焦抑制和焦点恢复。
- macOS 在主线程运行 `NSOpenPanel`，复用现有保存对话框的 `lower_floating_windows` / `restore_levels` / `restore_previous_app` 逻辑；Windows 在合适 STA 上用带 owner HWND 的 `IFileDialog` / `GetOpenFileNameW`，owner 由宿主提供。不能在线程池随意调用 GUI API。
- 原生对话框是阻塞式的：打开期间 UI 线程不跑 `poll()`/`ui()`，所以热键隐藏、禁用功能、退出都要等对话框关闭后再处理。文档明确**不尝试从其他线程强行关闭系统对话框**；如果后续需要"打开期间仍可操作搜索页"，改用非模态 sheet（v2 再评估）。
- 对话框打开前，宿主设置自己的 `dialog_open()` 状态；关闭后设置 `just_closed_dialog`，`handle_focus` 据此跳过首个失焦。搜索模块不再传 `suppress_hide_on_blur`。宿主实现 `pick_folder` 时复用 `floating::DIALOG_OPEN`，并把 `just_closed_dialog` 从 `HubApp` 字段提升为平台层可见的标志（或由调用 `pick_folder` 的 wrapper 设置），保证搜索页和现有保存对话框共享同一套失焦保护。
- 对话框返回错误（权限、卷离线、被系统取消）时，模块显示提示，不改变现有根/范围。
- 支持把目录拖入范围控件；拖入根列表时按"添加根"校验，不能静默扩展索引范围。文件拖入按父目录处理。

### 5.5 搜索设置面板

齿轮只进入模块内设置页；不跳转 Hub 通用设置。

| 分组 | 配置 / 操作 |
| --- | --- |
| 索引根 | 添加、移除、启用/停用、卷/相对路径、离线/权限状态、条目数 |
| 排除 | glob 列表、卷相对 ignore_paths、规则大小写；内置不可删除排除项只读展示 |
| 索引行为 | lazy_start、watch、continue_when_hidden、scan_app_bundles、persist_index |
| 运行控制 | 暂停并释放内存、恢复索引、重建、立即保存、清除缓存；显示任务状态 |
| 搜索行为 | 默认 Aa、100ms 延迟、空查询、max_results、高亮 |
| 搜索库 | 记录历史开关、清除历史、保存搜索管理 |
| 界面 | Floating/Pinned、窗口尺寸、重置列宽/排序 |

索引配置使用"应用"/"还原"草稿，Service 根据差异决定剪枝、补扫或掩码，不一律全根重建。界面偏好经同一 ApplySettings 通道即时生效并合并写盘。保存失败显示"未保存"并允许重试，不能让 UI 和后台长期各持一份配置。

### 5.6 主题与组件

- 搜索 UI 不依赖 `devtools-hub::ui::style::Palette`；从 `ui.visuals()` 取颜色：强调色用 `hyperlink_color`，弱文本用 `weak_text_color()`，隔行底色用 `faint_bg_color`，选中行用 `selection.bg_fill`，警告用 `warn_fg_color`。
- 字体、字号、圆角由主模块安装的 egui 全局样式提供；搜索模块不重复安装字体。
- 搜索页底色由宿主用 `window_fill`（不透明）绘制，不跟随窗口的玻璃/半透明材质；这是可读性要求，不是主题分叉。
- 在独立测试中，搜索模块使用 egui 默认样式即可显示。

### 5.7 egui 实现约定

- 表格用自定义虚拟化组件（参考现有 `panel/cards.rs` 的 `ScrollArea::show_viewport`），不引入 `egui_extras`。
- 表头固定；列宽拖拽命中区 ±3px；使用 `CursorIcon::ResizeHorizontal`。
- 中间截断用字体布局测量后二分插入省略号，避免按字符数截断导致宽度不准。
- 图标异步加载：`ui/icons.rs` 维护请求队列，只对可见行请求；工作线程调用 `ports::IconProvider`，完成后通过通道回到 UI 线程并缓存纹理；缓存上限 512，按"扩展名 / 目录 / 路径"分组。占位文件只能用扩展名通用图标。
- 只对可见行请求路径、元数据和图标；滚动时用 LRU 保留最近约 2000 行，并绑定结果代际。
- 数字列用等宽字体；高亮只对可见行按需计算（见 §7.8）。
- 表格行/列至少提供 `accesskit` 标签；完整无障碍留给后续版本。
- 不使用 `egui_extras`、`egui_table` 等额外表格依赖，避免样式和行为分叉。

### 5.8 窗口模式（只作用于搜索页）

现状是单个 eframe 窗口：macOS 上被换成 `NSPanel`（`NonactivatingPanel` + `NSPopUpMenuWindowLevel` 101 + `CanJoinAllSpaces`），Windows 上创建为置顶的 toolwindow，所有页面共用。v1 不新增独立视口，但搜索页需要和剪贴板面板不同的两个窗口行为：

| 模式 | 行为 | 实现 |
| --- | --- | --- |
| `Floating` | 置顶、失焦隐藏（与现有剪贴板面板一致） | 现有 `panel::show` 行为，`hide_on_blur=true` |
| `Pinned` | 置顶、不随失焦隐藏、只存在于打开时的桌面 | `hide_on_blur=false`；集合行为 `MoveToActiveSpace` |

关键边界：

- 进入 `Page::Search` 时，宿主调用 `panel::set_search_space_behavior(true)`：macOS 把集合行为从 `CanJoinAllSpaces` 切到 `MoveToActiveSpace`，这样 Pinned 的大窗口不会跟着用户去每个桌面、也不会盖在全屏应用上；热键唤起时窗口被拉到当前桌面。离开搜索页恢复原集合行为，剪贴板面板行为不变。Windows 没有 Space 概念，no-op。
- 窗口层级仍是 101：Floating/Pinned 都是置顶。v1 不要求"被普通窗口覆盖"，那是 v2 的独立视口。
- macOS 的窗口类仍是 `NSPanel`，`NonactivatingPanel` 和 `_setPreventsActivation` 在 attach 时设置；搜索窗口点击仍不激活本应用，因此文件打开/显示走系统动作，不依赖应用前台。
- Windows 的 `with_taskbar(false)` 是创建参数，运行中无法恢复任务栏按钮；v1 搜索页不会出现在任务栏，Pinned 靠置顶和热键找回。
- 无标题栏：拖动只在顶栏空白区；`Pinned` 下用户把窗口拖到哪就停在哪，不自动居中。`Floating` 每次唤起按 `ScreenCenter` 定位，沿用现有 `panel::show`。
- 搜索页由宿主画不透明底（§5.6），避免 1100×700 的玻璃面板压住桌面内容导致表格不可读。
- 如果要求 Dock / 任务栏按钮、标题栏、搜索窗口和剪贴板面板同时存在，应使用**独立视口**（v2）：在搜索模块内部用 `ctx.show_viewport_deferred` 创建第二个窗口，主模块只增加显示入口。v1 不承担这部分工作量。
- 验收要覆盖：搜索→剪贴板/JSON/OCR/设置的所有路线恢复目标窗口尺寸、最小尺寸、失焦策略和集合行为；Pinned 下切换桌面再按热键能回到当前桌面。

### 5.9 中文输入法（IME）

- 组合期间不触发查询：搜索框收到 `egui::Event::Ime(ImeEvent::Preedit { .. })` 且文本非空时，设置 `composing=true`，暂停 debounce、停止发起新查询、保持旧结果；收到 `ImeEvent::Commit` 或空的 `Preedit` 后 `composing=false`，再按 debounce 搜索一次。
- `Esc` 在 `composing=true` 时交给输入法 / `TextEdit` 处理，不清空查询、不请求宿主隐藏；只有在非组合状态才执行"第一次清空、第二次隐藏"。
- `Enter` 同理：组合状态下交给输入法上屏，不提交搜索；上屏后的 `Commit` 触发搜索。
- egui 的 `TextEdit` 内部有 `is_ime_composition`，但不是公开 API；这里通过输入事件自行维护状态。egui 0.36 通常把 preedit 画在组合区、不写入绑定字符串，但不同平台/输入法的 `Preedit` / `Commit` 序列不完全一致；不要依赖"文本没变就不会查询"的偶然行为。
- 需要注意 `egui-winit` 文档列出的平台差异：有的平台会先发空 `Preedit` 再 `Commit`，有的会重复空 `Preedit`；状态机必须能处理这些序列。
- 验收：macOS 拼音、Windows 微软拼音各手工验证一次：组合期间结果不刷新、不闪烁；上屏后只发一次查询；组合中按 `Esc` 只取消候选，不隐藏窗口。

## 6. 搜索设置模型与迁移

### 6.1 配置示例

主设置只新增以下两项（示例为 Windows 默认热键）：

```json
{
  "search_enabled": false,
  "search_hotkey": "Alt+Shift+F"
}
```

模块独立设置示例，路径为 macOS 示意值，实际默认根由平台解析用户主目录生成。**从 version 1 开始**：搜索功能此前没有发布过，第二版的 version 2 和"v1 迁移"是死代码，不保留。

```json
{
  "version": 1,
  "roots": [
    {
      "location": {
        "volume_id": "mac-volume-uuid",
        "relative_path": "Users/me",
        "fallback_absolute_path": "/Users/me"
      },
      "enabled": true
    }
  ],
  "exclude_patterns": [
    "node_modules/",
    "target/",
    "DerivedData/",
    ".git/",
    "Library/"
  ],
  "exclude_case_sensitive": false,
  "ignore_paths": [
    {
      "volume_id": "mac-volume-uuid",
      "relative_path": "Users/me/Downloads/Archive",
      "fallback_absolute_path": "/Users/me/Downloads/Archive"
    }
  ],
  "scan_app_bundles": false,
  "watch": true,
  "lazy_start": true,
  "continue_when_hidden": true,
  "index_paused": false,
  "persist_index": true,
  "debounce_ms": 100,
  "default_case_sensitive": false,
  "show_all_on_empty": true,
  "max_results": 200000,
  "highlight": true,
  "history_enabled": true,
  "history_limit": 100,
  "permission_onboarding_completed": false,
  "ui": {
    "window": {"size": [1100.0, 700.0], "mode": "floating"},
    "columns": {"name": 260.0, "path": 450.0, "size": 100.0, "modified": 155.0, "created": 155.0},
    "sort": {"column": "path", "descending": false}
  },
  "history": [],
  "saved_searches": []
}
```

- `roots[].location` 是 core 的 `RootLocation`；`settings` 依赖 core 的纯数据类型，不反向。
- `Library/` 含 `/`，按根相对路径匹配，只命中主目录顶层的 `Library`；这是 macOS 默认排除项。用户可以在设置里删除它，但设置页要说明"会显著增加扫描时间和权限提示"。
- Windows 默认排除 `AppData/Local/Temp/`、`AppData/Local/Packages/` 及能可靠解析的包管理器缓存；不默认排除整个 `AppData/Roaming`。
- `permission_onboarding_completed` 仅表示看过引导，不表示系统授权永久有效。内置排除项运行时派生，不依赖用户 JSON 保存，因此不能通过删字段解除。
- `index_paused=true` 表示用户主动暂停并释放索引，重启也不自动恢复；"恢复"改为 false。隐藏时 `continue_when_hidden=false` 只做会话暂停，下次显示自动恢复，不改 index_paused。`persist_index=false` 时既不加载旧快照也不自动保存，旧文件保留至显式清除。

### 6.2 配置存储

- `#[serde(default)]` 补齐缺失字段，校验范围、路径、根冲突、尺寸和模式；未知未来版本不得直接覆盖，只读加载并在界面提示。
- 同目录写临时文件、flush/fsync、平台原子替换；Windows 使用支持替换已存在目标的语义（`ReplaceFileW` / `MoveFileExW(REPLACE_EXISTING)`），不能先删除有效文件再 rename。
- 解析损坏隔离为 `.corrupt.<timestamp>`，保留原始字节，默认配置恢复并在模块界面提示。
- 不存在"从第二版配置迁移"：搜索功能没有发布过。将来引入新版本时，再按版本号写迁移。
- 旧缓存按旧版本视为可重建缓存，完成新配置保存后重建，不把不兼容节点数据当作新格式加载。

### 6.3 变更策略

| 变更 | 应用行为 |
| --- | --- |
| 窗口模式/尺寸、列宽、排序 | 即时更新视图；策略变更通知宿主；合并写盘 |
| 默认大小写、延迟、空查询、上限 | 更新配置并使当前查询按需重跑 |
| 添加根 | 先验证身份与包含关系，通过后仅扫描新根 |
| 移除/停用根 | 取消任务、关闭句柄、结果失效；移除可清理对应快照，停用保留配置 |
| **新增**排除规则 | 在内存分批把命中节点及后代标为 `excluded`（或查询时检查祖先掩码），同步扫描/事件过滤；不重建无关根 |
| **删除**排除规则 | 清除受影响子树的 `excluded` 掩码；从未扫描过的子树补扫，通配规则无法缩小范围时校验受影响根 |
| watch 从关到开 | 先建立监听并校验停用期间变化，不能仅打开 watcher 就称数据最新 |
| watch 从开到关 | 停止 watcher、保留当前索引；重新打开时按"从关到开"校验 |
| 修改包递归策略 | 补扫/剪枝受影响包目录 |
| 暂停并释放 | 取消工作、停止 watcher、按 persist_index 保存、释放索引/结果/缓存 |
| 清除索引缓存 | 停止相关写入、清理模块快照；在线实例标脏并按当前运行策略重建 |

每次配置变化递增 `config_revision`，扫描任务携带该版本，旧任务不得在应用新规则后覆盖当前根。排除规则的新增/删除走 `excluded` 掩码，不产生 ID 复用，也不等于删除节点（见 §7.1）。

## 7. 索引与查询

### 7.1 紧凑数据、节点状态和生命周期

- 根内 `NodeId=u32`，`NameId=u32`，会话内 `RootId=u16`；`ResultId=u64` 打包 RootId/NodeId。u32 上限约 **42.9 亿**，预留哨兵后可用数量更少；溢出必须拒绝继续并提示。
- 节点表用 SoA：parent、name、kind/flags、size、mtime、btime、元数据有效位、状态位分别连续存储；目录关系与名字倒排用平面缓冲区/独立目录表，不给每个文件分配 ThinVec/Box。
- 原始文件名、NFC 匹配文本及不区分大小写文本存入连续 arena，以偏移/长度引用；每根独立驻留，不跨根共享，方便离线根释放。三种文本常常完全相同（CJK 下 raw == NFC == lower），**共享同一块 arena + 标志位**，只在不同时存第二/第三份。
- 原始 Unix 字节/Windows UTF-16 名字需可无损保存；显示可以有替代字符，但打开、reveal、路径拼接必须使用原始编码。不能通过 NFC 文本定位真实文件。
- v1 不区分大小写明确采用"Unicode NFC → Rust Unicode lowercase → NFC"，不是完整 Unicode case folding；例如 ß 与 ss 不保证等价。Aa 开启仅做 NFC，保留大小写。正则 Aa 行为单独使用 regex 的 Unicode 忽略大小写语义，并测试差异。
- 创建时间命名 `btime/created`，不再使用含混的 ctime；不支持或失败为 Unknown，不能用 Unix 状态变化时间冒充创建时间。
- **节点状态拆成三种**，不能共用一个"墓碑"位：
  - `deleted`：磁盘上已删除，同一 `index_epoch` 内不复用 NodeId；LoadRows 不返回该行，已显示的行标为"已删除"且不可打开，下一次查询后替换；
  - `excluded`：被排除规则掩码，可恢复；查询不返回，LoadRows 不返回；解除规则后恢复可见，旧 ResultId 仍指向同一个文件；
  - `pending_remove`：批量删除中的临时隐藏，几毫秒后转为 `deleted`；LoadRows 不返回。
- 重建/压缩后分配新 `index_epoch`，旧结果全部失效。墓碑/掩码占比超过约 20%，或空闲 10 分钟以上时，后台压缩并切换 epoch；压缩不能改变文件身份。
- 结果批次绑定 `session_id`、`query_generation`、`root_id`、`index_epoch`，**不绑定当前 revision**。LoadRows 逐行校验：`deleted` 显示"已删除"或省略，`excluded`/`pending_remove` 不返回，其他行允许显示稍旧的大小/时间；结果集与排序由 revision 失效（§7.9）。
- 打开结果前在 Service 再校验身份/代际和当前可访问性；执行瞬间仍可能被外部删除，系统错误正常反馈。

### 7.2 内存与预算

100 万条目暂定：节点主体 ≤60MB，名字及文本目标 ≤25MB，单份 100 万 u64 结果约 8MB，空闲 RSS 相对未加载索引增量 ≤150MB，压缩快照目标 ≤15MB。

这些是预算，不是 SoA 必然达到的结论；原始/NFC/小写文本、目录关系、代际表、图标、排序和分配器开销均计入 RSS。名字 arena 用"相同文本共享 + 标志位"后，典型 CJK 主目录的三份文本接近一份；M0 必须用真实主目录量 RSS，若 25MB 不成立就改预算，而不是隐藏失败。高亮映射按需计算，不计入常驻内存。

旁路重建及保存时的临时副本单独统计峰值，目标 ≤300MB 增量，最多一个根做重型重建/快照复制；不按"200 万约 1.8 倍"无依据外推。结果批次代际表、排序临时数组、图标纹理都计入峰值。

### 7.3 扫描与元数据

- 专用扫描池，SSD 默认最多 min(4, 可用核数) 个 worker；机械盘、介质未知或外置盘默认 1，可据实测调优。网络卷 v1 拒绝。
- 先排除目录再枚举内部；每个纳入节点在扫描时读取大小、修改/创建时间、类型、占位属性；失败显式标 Unknown。查询阶段禁止按候选逐个 stat。
- 判断"跨卷挂载点"直接用本次 `lstat` 已返回的 `st_dev`（macOS）/ reparse tag（Windows），不额外对每个目录 `statfs`；跨卷目录默认不递归，需显式添加根。符号链接不跟随。
- Windows 优先 `DirEntry::metadata()` 利用枚举携带元数据，不为普通节点另按完整路径重复 stat；仅必要属性补取，并做计数。
- macOS M0 对比 read_dir+lstat 与 getattrlistbulk，按真实主目录总耗时、系统调用数、元数据准确性和错误处理选择实现，不能等上线后再决定。
- 不读取文件内容；不跟随 symlink/junction/reparse 目录递归；.app 内部默认跳过，包本身可搜索。
- 云占位文件只记录元数据，用平台属性识别（macOS `SF_DATALESS`，Windows `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` / cloud reparse tag），禁止后台内容/缩略图/路径图标访问。若某提供商连枚举都会下载，提示并排除相应目录。
- 全量扫描在旁路建新根；建立 watcher 后开始扫描，缓存期间事件并在发布前重放到明确检查点，再原子替换。事件丢失则不能发布为 Ready，进入需校验状态。
- 所有任务携带取消标志和 config_revision；关闭、卸载、排除变更时及时释放目录迭代器，禁止带已取消任务重新打开设备句柄。

### 7.4 排除规则

`node_modules` 匹配任意层级同名项，`node_modules/` 只匹配目录；`*.log` 匹配 basename，`**/build/**` 匹配根相对路径并剪枝。规则不含 `..`，无重新包含语法；`exclude_case_sensitive` 独立于查询 Aa。

扫描和事件都检查路径及祖先；命中排除项不会 walk 内部，但仍可能收到底层系统事件，不能声称能避免 watcher 缓冲区溢出。

默认可编辑排除（macOS）：

```
node_modules/
target/
DerivedData/
.git/
Library/
```

Windows 默认：`node_modules/`、`target/`、`.git/`，外加 `AppData/Local/Temp/`、`AppData/Local/Packages/` 和能可靠解析的包管理器缓存（npm/pnpm/yarn 等）。具体目录在设置中展示；不默认排除整个 `AppData/Roaming`，以免过度隐藏用户配置。

排除整个 `~/Library` 是产品决策：它通常有几十万文件、大量 TCC 保护目录（Mail、Messages、Safari、Containers），保留会显著拖慢首扫并制造授权提示。设置页要提供"删除该规则"的入口和成本说明；M0 用真实主目录统计各二级目录节点数，若不排除 Library 也能达标，再考虑放宽。

内置不可删除忽略项：`<data_dir>/search`、当前实例及同应用搜索缓存根（覆盖 settings、index、tmp、corrupt 文件）。平台以规范化路径/身份处理别名，扫描、增量、补扫均适用，防止保存快照触发自索引。

ignore_paths 与索引根采用同一 `RootLocation` 卷相对格式；挂载点变化后重算绝对路径，不能留下一套过期绝对忽略路径。

### 7.5 根身份、重叠和卷状态

`RootLocation = (volume_id, relative_path, fallback_absolute_path)`；绝对路径仅用于展示和无稳定卷身份时的兜底。已有 volume_id 却找不到该卷时标离线，不能凭相同盘符/挂载路径绑定另一块盘。

`root_key = SHA-256(带格式版本的规范编码(volume_id, 根相对卷路径))`，使用完整十六进制；无稳定卷 ID 时用明确标记的规范绝对路径身份。根文件头还要核验完整身份，不能只信文件名。路径规范化遵循卷的大小写特性；身份键不盲目 NFC/全小写，以免合并不同文件系统路径。

添加根时解析规范路径与卷身份，检查重复、父子包含、路径别名；v1 **拒绝重复/嵌套根并提示已覆盖的根**，不自动维护双向排除。离线根先按已保存卷相对位置检查，无法确认的根标 PendingValidation，挂载校验通过前不启动。

根状态：Dormant / AwaitingConsent / Offline / PendingValidation / Scanning / Verifying / Ready / Dirty / Paused / Ejecting / Error。状态和配置 enabled 分开。

- 在线：定位到当前挂载点再加载该根快照或扫描。
- 卸载：先取消任务、释放句柄，再移除在线结果，提升代际；保留配置与磁盘快照。
- 同卷重新挂载：恢复卷相对路径，按平台恢复流程校验。macOS `/Volumes/Disk` 变成 `/Volumes/Disk 1` 或 Windows 盘符改变均不改变根身份。
- 卷事件通知与低频轮询互为补充；没有运行中根时可停轮询。关闭主功能后设备线程必须退出。

### 7.6 快照、检查点与崩溃一致性

```text
<cache_root>/search/<instance_key>/index/
├─ manifest.json
└─ roots/
   ├─ <root_key>.<snapshot_generation>.bin
   └─ ...
```

每根快照包含 magic、格式版本、完整根身份、配置/排除指纹、index_epoch/revision、原始名字及匹配文本、节点列、节点状态、有效 rank、**已完整应用的 last_event_id 和事件流身份**、校验和。macOS 检查点必须与节点数据在同一个一致性捕获中产生；不能另写 manifest 的"最新收到事件 ID"。Windows 检查点为空。

写入顺序：捕获一致数据 → 同目录 tmp → 编码/压缩/校验 → fsync → 提交唯一代际文件 → 原子更新 manifest 指针 → 回收未引用旧代文件。manifest 仍指向旧文件时旧快照保持有效；新文件不能覆盖 manifest 正引用的同名文件。manifest 损坏时按设置根列表重建；孤立文件可清理，不凭残缺清单猜身份。

快照捕获采用每根读锁下复制紧凑数据及检查点，随后释放锁在后台序列化；这是明确的 v1 选择。一次只复制一个根，记录复制停顿和峰值，不声称无锁/零复制。M0 若超预算，必须修订为有版本保障的分块机制，不能分块读出不同 revision 的混合快照。

按根独立加载/损坏隔离/删除。离线根不加载；指纹不匹配不得先暴露违反新排除规则的旧结果。每根脏数据最多约 10 分钟安排一次保存，持续写入也要有最大等待期限，避免纯 debounce 永远不保存；退出/暂停按 persist_index 尝试保存，退出走 `shutdown_blocking` 的有界等待。

保存失败或退出超时：保留旧快照和 manifest，不清空；下次启动按旧检查点校验/回放。缓存目录里的孤立代际文件和超过 30 天未使用的实例目录在启动时清理。

### 7.7 增量、重启和重建

- watcher 事件进入有界队列，100–300ms 合并；创建/删除/重命名对受影响目录重新枚举，元数据变化更新已缓存字段。事件丢失、队列溢出、无法识别重命名或根变更将根标 Dirty。
- 更新以逻辑一致小批提交，写锁目标持有 ≤5ms，目标需测量；大型子树删除通过 `pending_remove` 先整体隐藏，提交后转为 `deleted`，不能暴露半更新 parent 链。
- 每次提交增加 revision、发 Dirty；Service 对当前查询防抖约 150–300ms 重跑，持续事件设置最大等待约 1s。UI 不自行维护索引，过期 SearchDone/Rows 丢弃。
- **revision 只用于结果集和排序失效，不用于拒绝行内容**：只要 session/query_generation/root/epoch 匹配、行未被删除/排除，就允许显示旧行。这样构建目录持续写入时结果区不会反复空白。
- macOS 从快照记录的事件检查点追赶；仅在历史可回放、事件流身份及配置匹配时使用。处理完检查点之前的全部事件才前移 last_event_id；失去历史则全根校验。
- **Windows 每次冷启动/恢复监控都校验**：旧快照可立即搜索但标 Verifying；先建立监听，再后台全量重扫、回放扫描期间事件、原子替换。ReadDirectoryChangesW 不能补回应用未运行期间的事件。
- 溢出/丢事件立即标"更新可能滞后"；同根自动完整重建启动频率限制为 5 分钟最多一次，等待期间保留 dirty 状态并安排下一次，不能丢弃重建需求。手动重建可覆盖限流。
- 根重建期间旧结果保留并显式标旧，其他根正常搜索；根离线/删除则立即撤出结果。发布时同时核对任务取消、配置版本和卷身份。
- 墓碑/排除掩码占比超过约 20% 或长时间空闲时后台压缩；压缩期间旧 epoch 继续可搜索，完成后原子切换并让旧结果失效。

### 7.8 查询语法和求值

v1 确定移植 `everything-mac-syntax` 的适用解析部分到内部核心 `query/syntax/`，求值器自建。M0 核对实际版本、许可证和语法边界；不得把外部项目 README 的功能列表当作本模块已实现能力。

| 语法 | 含义 |
| --- | --- |
| report / "annual report" | 文件名子串 / 保留空格的短语 |
| report*.txt | 名字通配符，* 任意长度，? 单字符 |
| a b / a \| b / !a / (a \| b) !draft | AND / OR / NOT / 分组 |
| project/report | 路径分段匹配 |
| ext:pdf;txt / type:picture | 扩展名 / 明确的扩展名类型组 |
| file: / folder: | 文件 / 目录 |
| size:>1MB / size:100KB-5MB | 大小过滤 |
| dm:today / dc:pastweek | 修改 / 创建时间过滤，按本地时区边界 |
| infolder:~/Documents / parent:~/Downloads | 全部后代 / 直接子项 |
| regex:^report | 对名字执行正则；非法表达式显示错误 |

补充约定：

- 大小单位固定为 1024 进制：`1KB=1024B`、`1MB=1024KB`，文档和设置页显示一致。
- `type:picture` 的映射表固定在 `query/types.rs`：png、jpg、jpeg、gif、bmp、webp、tiff、heic、avif；后续扩展只改表，不在解析器里硬编码。
- `infolder:` / `parent:` 指向未配置索引的目录时返回空结果并提示"该目录不在索引范围内"，不报错、不自动加根。
- 入口带来的原始文本走 `query_is_literal`，按字面短语匹配，不解析 `|`、`!`、`(` 等语法；用户手动输入时才按语法解析。
- 不明确的斜杠锚定写法不在 v1 中隐式解释；输入不支持的过滤器报错，不悄悄忽略。移植后保留上表固定测试向量，M0 给出精确的引号转义、优先级和日期区间文档。

- 名字词先匹配驻留名字，再展开节点；大小/时间只读取内存元数据，Unknown 不命中需要该字段的过滤器，排序统一置后。
- 路径词先匹配最后一段名字，再沿 parent 链逐段验证；不为百万条目预先拼完整路径。
- infolder/文件夹范围解析成目录节点，以对应 revision 的 DFS `[tin, tout)` 判后代；rank 失效时退回 parent 链，候选超过 10 万或明显超时则先显示"排序中…"并在后台重建 rank，不能让一次查询长时间占满查询池。
- parent 直接比较父节点。
- 显式 path: 词如需支持，由 AST 单独表示并沿路径链求值；未完成实现前不在 UI 语法帮助宣称支持。
- Aa 使用 §7.1 的文本规则；高亮仅支持字面词和短语。**高亮映射只对可见行按需计算**：把匹配区间从 NFC 文本映射回原始字节时逐字符走一遍，不把 normalized→raw 偏移表常驻内存；按字符区间而不是字节偏移切分，避免 NFD 重音字符被切半。
- 所有查询可取消。正则单独设时间预算、复杂度/长度限制；不把正则混入普通子串的 30ms 承诺。

### 7.9 排序、分页、上限

- 默认路径序：根按显示路径稳定排序，根内按 children 名字顺序 DFS 生成 rank；NodeId 顺序绝不能作路径顺序。
- rank 与结构 revision 绑定，变更后失效，后台重建并验版本后发布；不得用旧 rank 判断新树后代。rank 重建期间显示旧结果并标"排序中…"。
- 结果保存 ID 向量以及批次代际表，不保存每条完整路径。名字、大小、修改/创建时间排序在后台完成，排序键来自内存。revision 变化使当前结果集与排序缓存失效，Service 防抖重跑；行内容不因此被拒绝（§7.7）。
- max_results 默认 200,000，0 表示不限并提示资源开销。必须在**全部匹配集合**上按当前排序取前 N（top-k），不能先截断任意 N 条再排序；准确总数仍需遍历匹配集合。
- 只留当前查询/排序的一份有效缓存；旧任务及时取消，临时排序数组计入峰值预算。结构高频变化时 500ms–1s 合并重排，期间可显示旧列表并标记更新中。
- UI LoadRows 只请求可见区+overscan，最多 128 行，缓存约 2000 行并绑定 session/query_generation/root/epoch；分页只回填已缓存的大小/时间，不能顺手执行 stat。revision 变化不拒绝行，只有 epoch/会话/行状态不匹配才拒绝。

### 7.10 并发模型（固定方案）

```text
UI ──Command──> application::SearchService（协调与设置单写）
UI <──Event──── Service
                  ├─ 专用查询 rayon ThreadPool
                  ├─ 专用扫描 rayon ThreadPool
                  ├─ 每根 Arc<RwLock<RootIndex>>
                  └─ 平台 watcher / 设备消息线程 / 存储任务
```

- 查询按根/批次获取读锁，在专用池并行执行；记录 revision，批次间版本改变则取消重查，不把不同版本结果混成一次一致查询。
- 单根更新由串行应用器完成；磁盘 I/O、图标、文件对话框、压缩不得在写锁内执行。写锁批次目标 5ms；读任务分段释放并检查取消，避免长正则长期饿死写入。
- 扫描优先级：macOS utility QoS，Windows 线程后台模式；线程池总量有上限，不随根数无限创建 worker。watcher 回调尽快重新挂读取，重活交 Service。
- 命令：Search、LoadRows、SetSort、ApplySettings、AddRoot、RemoveRoot、PickFolder、Open、Reveal、Pause、Resume、Rebuild、Save、ClearCache、Shutdown。
- 事件：SettingsLoaded/Applied、IndexStatus、SearchDone、Rows、RootsChanged、Dirty、DialogState、Error、Stopped。所有异步回包携带会话/请求 ID。
- 原生对话框在主线程同步执行；期间不投递新的搜索/设置命令，关闭后按队列处理。`DialogState` 只用于 UI 展示和宿主失焦保护，不替代宿主自己的 `dialog_open()`。
- UI 每帧只消费有限事件；后台通过 waker 请求重绘，空闲时不持续排帧。
- 关闭：`shutdown()` 拒绝新任务并取消；`shutdown_blocking()` 额外 join 存储任务，期限到了返回 false，保留旧快照。

## 8. 平台适配

### 8.1 ports 与安全能力

核心端口包括 FsSource、WatcherFactory/WatcherHandle、VolumeSource、IconProvider、SystemActions、FolderPicker、SystemModalGuard、AtomicStorage；参数和回包都是纯数据。平台对象由组合根注入。

- `FolderPicker` / `SystemModalGuard` 的 trait 在 core::ports，**实现在宿主平台层**；模块平台不实现原生对话框。模块的测试用 fake picker 验证"选择→校验→应用根/范围"的流程。
- `IconKey` 使用 `Ext`、`Dir`、`Path(VerifiedLocalPath)`。VerifiedLocalPath 构造器非公开，仅元数据策略校验后产生；占位/未知提供商项只能生成通用 Ext/Dir 键。平台图标加载前还要复查占位属性。
- `SystemActions` 仅提供 open/reveal；删除 open_terminal。打开占位文件只能由用户明确动作触发并提示可能下载，后台索引/加载图标不能调用 open。
- `FolderPicker`、`SystemModalGuard`、`start_window_drag` 不把 NSWindow/HWND 暴露到 core；宿主实现负责层级、激活、焦点和拖动。
- `AtomicStorage` 负责平台原子替换细节，设置及快照通过端口调用。

### 8.2 macOS

- 使用 FSEvents per-root stream；权限和首次引导见 §5.3，检查点见 §7.6–7.7。
- NSWorkspace 通知从其专用 notificationCenter 订阅；WillUnmount 时取消该卷所有根的扫描、监听和路径图标任务，释放句柄。强行拔盘可能没有 WillUnmount，仍要处理 DidUnmount、I/O 错误和轮询。依据：[Apple willUnmountNotification](https://developer.apple.com/documentation/appkit/nsworkspace/willunmountnotification)。
- AppKit 图标/对话框操作遵守主线程要求；允许后台做 RGBA 后处理，不把所有 NSWorkspace/NSImage 调用一概放线程池。占位文件仅通用图标。
- 搜索页窗口行为：进入 `Page::Search` 时把集合行为切到 `MoveToActiveSpace`，离开恢复 `CanJoinAllSpaces | FullScreenAuxiliary | IgnoresCycle`；窗口层级保持 `NSPopUpMenuWindowLevel`。这是 `panel.rs` 的一个小函数，不是新的窗口模式。
- 原生对话框由宿主实现：复用 `floating::save_dialog` 的 `activateIgnoringOtherApps` → `lower_floating_windows` → `runModal` → `restore_levels` → `restore_previous_app` 流程；搜索页不需要再实现一套层级的降级。
- 发布要求稳定 bundle ID 与签名身份，生产包使用稳定 Developer ID 并验证升级后权限体验。当前 release workflow 仍存在无证书时临时签名回退，不能写成"已满足"。它是发布验收前置条件，由发布流程处理，不是将搜索平台实现移到主模块。依据：[当前 release.yml](https://github.com/zvae/devtools_hub/blob/679ab298f4b7d564616015c6daf435a17e40003e/.github/workflows/release.yml)。
- 开发临时签名可能反复请求授权，文档和测试结果区分开发构建与发布构建；不通过管理员提升绕过 TCC。

### 8.3 Windows

- ReadDirectoryChangesW 使用 overlapped I/O 与受限缓冲队列；专用线程尽快重新提交读取。缓冲大小需符合文件系统限制，不能简单无限增大；M0 用 npm install 事件风暴测溢出。
- **模块自有设备消息线程与隐藏窗口**，不借用 eframe 窗口。若使用仅消息窗口，必须验证能收到显式注册的 handle 通知；挂载广播由合适的隐藏顶层窗口/显式通知加轮询处理，不能假设消息窗口接收所有广播。
- 为实际持有的设备/目录句柄注册 `RegisterDeviceNotificationW(DBT_DEVTYP_HANDLE)`；维护卷到所有根/句柄的映射。
- 收到 DBT_DEVICEQUERYREMOVE 进入 Ejecting，停止发起新 I/O，取消 pending 读取、停止扫描并关闭 watcher、目录迭代器及其他模块持有的相关句柄；不能仅排队取消后立即声称完成。回调不做重扫/快照重活，资源释放协议须在 M0 实机验证。
- DBT_DEVICEQUERYREMOVEFAILED 恢复设备可用状态、重新打开并注册必要句柄、启动校验；REMOVEPENDING/REMOVECOMPLETE 做幂等清理。轮询只补充到达/离线检测，**不能替代安全移除协议**。依据：[Microsoft 移除设备请求处理](https://learn.microsoft.com/en-us/windows/desktop/DevIO/processing-a-request-to-remove-a-device)。
- 图标按物理像素请求，通过系统图像列表或合适 Shell 图标接口获取，缓存包含像素尺寸/DPI；不能只取固定 16px 图标再放大。占位文件绕过路径图标接口。
- 内部保留 UTF-16/PathBuf 和长路径能力；显示可转换，操作不能使用 lossy 字符串。Shell 打开/显示使用参数化 API，避免拼接命令字符串。
- 原生文件夹选择由宿主实现：`IFileDialog`（STA）或 `GetOpenFileNameW`，owner 必须是宿主窗口；不能传 NULL，否则对话框会被置顶面板压住。
- 退出：托盘"退出"不再 `process::exit`，改发 `AppEvent::Quit`；`on_exit` 有界等待。更新重启在 `run_native` 返回后重新拉起，避免跳过保存。

## 9. 错误、隐私与恢复

- 配置、权限、扫描、离线、监听丢失、快照、平台失败、对话框失败分别分类；单目录错误计数继续，单根失败不拖垮其他根。
- 日志 target 为 devtools_search，普通级别仅计数/耗时/状态；调试路径避免写入常规日志。历史明文风险在模块设置说明。
- 不上传数据、不读取内容、不启动网络服务。搜索结果可能是旧快照，打开时失败按正常外部变化处理。
- 所有缓存和设置仅写当前用户目录，不放宽权限。清理只针对本模块已确认的缓存路径，不根据根目录配置删除用户文件。
- 正常退出有界保存；崩溃/强杀不保证保存，旧快照保持有效。关闭模块后等待线程、设备通知、对话框、定时器释放完成；取消后的晚到结果通过 session_id 丢弃。
- 原生对话框失败（权限、STA、层级）只影响本次选择，不把搜索页标为 Error；UI 提示后继续可搜索。

## 10. 测试与验收

### 10.1 自动化覆盖

- 配置：主开关缺省 false、录制热键关闭时不注册；模块配置版本 1 的默认值、损坏恢复、单写入方、原子写失败、缓存实例隔离、未知未来版本只读。
- 分层：核心独立编译、依赖白名单、AST 越层导入、settings→core 单向、Hub 不直接依赖内部核心。
- 查询：语法、Aa/NFC/韩文组合与分解字符、非 ASCII 小写差异、Unknown 元数据、路径链/DFS fallback、top-k 与全量排序一致、`query_is_literal` 不解析布尔、`infolder` 越界返回空、size 单位。
- 索引：三种节点状态（deleted/excluded/pending_remove）、删除后 ID 不复用、排除规则新增/删除的掩码与恢复、旧代 LoadRows/打开请求拒绝、**持续 revision 变化时行仍可显示**、更新后当前查询自动刷新、压缩切换 epoch。
- 根与排除：同卷多根快照不同名、嵌套/别名拒绝、卷挂载点变化、离线卷同盘符不可误认、自身目录永不索引、默认排除 `Library/` 生效且可删除、跨卷挂载点不递归。
- 快照：数据和事件检查点一致，写 tmp/代际文件/manifest 各阶段崩溃，单根损坏隔离，配置变化时旧扫描不得覆盖新根，退出超时保留旧快照，孤立代际文件清理。
- 生命周期：默认关闭、开启懒加载、后台 warm_up、隐藏暂停、切页恢复、禁用中立即重启、退出前尺寸保存、`shutdown_blocking` 超时、取消后晚到事件、托盘/热键/快捷面板入口不绕过 `set_page`。
- 对话框：fake `FolderPicker` 的取消/错误/成功路径；模态期宿主失焦保护；对话框关闭后的首个失焦不收起；禁用/退出在对话框关闭后继续。
- UI：合成 1 万行虚拟化、焦点相关 Enter、历史不按字符写、IME 序列、搜索页不透明底、拖动只作用于顶栏；Pinned 下切换桌面为手工验收。
- watcher 实机测试可标 ignore，但发布必须手工运行；平台属性可用 fake source 测试占位项不请求路径图标。

### 10.2 发布验收清单

1. 两平台 `cargo check --workspace`、`cargo test --workspace` 通过；核心独立检查、分层检查通过。
2. 主设置仅新增 search_enabled/search_hotkey；主界面没有根目录/排除/索引/列宽/历史设置；所有搜索业务实现位于 devtools-search 子树，宿主只实现 FolderPicker/SystemModalGuard/start_window_drag 机制。
3. 关闭时无 SearchModule、搜索线程、搜索文件 I/O、搜索热键注册；关闭后残留入口事件不能启动搜索。默认关闭对启动耗时/RSS 的差异应在基线噪声范围，M0 记录至少 20 次对比。
4. 启用且 lazy_start=true、尚未打开时，不加载索引、不启动 watcher；只有轻量配置和 facade 开销，单独记录。
5. 外置盘扫描/监听中可正常安全弹出；每个平台实测安全弹出、取消弹出、强拔、重插、多个根同卷、盘符/挂载名变化。Windows 不接受"仅轮询"验收。
6. Windows 退出后新增/删除/重命名文件，再启动：先显示校验状态，完成后结果一致；校验期间新事件不丢。macOS 回放断点及不可回放回退均验证。
7. macOS 首次权限流程、拒绝后部分可用、重新授权、签名稳定发布升级分别验证；拒绝目录有数量和详情；首次授权期间系统弹窗不被搜索面板遮挡。
8. 文件夹对话框不导致面板意外消失；退出/禁用会等待对话框关闭并回收回调；搜索→剪贴板/JSON/OCR/设置所有路线恢复目标窗口尺寸、最小尺寸、失焦策略和集合行为。
9. 同卷不同根无快照覆盖；删除根不重写其他根，单根损坏不重建其他根；快照保存不产生自索引结果。
10. 百万条目空闲 RSS 增量 ≤150MB；临时峰值另测，目标 ≤300MB；watcher 无事件时 CPU 目标接近零，记录空闲 60 秒平均值；名字 arena 的实测值回填文档。
11. IME 组合期不查、Commit 后合并查询，Enter/Esc 不误操作；2x 缩放图标清晰；iCloud/OneDrive 扫描与图标加载不触发下载。
12. 纯增量结果和全量重建一致；删除项不会因 ID 复用变成另一文件；排除项解除后恢复为同一文件；精确 top-k、总数和分页代际正确；持续写入时结果区不反复空白。
13. 正常退出：托盘退出/更新重启走 `on_exit`，尺寸与快照有界落盘；强杀后下次启动能按旧快照/校验恢复。
14. 搜索页窗口行为：Pinned 下切桌面不跟随、热键拉回当前桌面；Floating 失焦隐藏；无标题栏时顶栏可拖动；搜索页不透明底、表格数字不跳动。
15. 托盘/热键/快捷面板三条入口在搜索页可见、其他页可见、隐藏时分别验证；托盘项在关闭时禁用、开启时可用。

### 10.3 性能口径

M0 固定参考机器并记录 OS、CPU、内存、SSD/HDD、数据集、文件数、冷/热缓存、构建模式；报告合成百万节点和真实主目录两组数据。以下是预研目标，M0 后必须用实测值确认或明确修订，不能当作测试已通过。

| 场景 | 目标与口径 |
| --- | --- |
| 3 字符词、多词 AND、size 过滤 | 百万节点热索引求值 p95 ≤30ms；不含输入 debounce、UI、磁盘 I/O及排序 |
| 单字符、路径词、infolder | 单独记录 p95，初始预算 ≤100ms；含精确计数，不能只测提前截断；rank 失效场景单列 |
| 普通查询到首屏 | 含 top-k/排序/128 行回包，p95 ≤150ms；路径 rank 重建单列 |
| regex | 独立查询集，初始 p95 ≤200ms、可取消；超预算显示状态，不能阻塞 UI |
| 首次扫描 | 参考本地 SSD、100 万可访问且未排除节点，初始目标 ≤120s；真实主目录另报，权限等待不计扫描但单列 |
| 增量负载 | npm install/批量解压压测，统计溢出次数、写锁持有时间、查询延迟、重建限流与最终一致性 |
| 名字与状态内存 | 单独报告 raw/NFC/lower arena、状态位、倒排、rank、结果缓存；25MB 目标未达标就修订 |

每组报告样本量、p50/p95/max 和失败数；不同查询集合不能混算一个漂亮的 p95。首次扫描、正则和内存未达标属于 M0 决策结果，应调整结构/目标，不隐藏失败。

## 11. 实施计划

| 阶段 | 交付 | 基础人天 |
| --- | --- | --- |
| M0a 结构与性能 | 临时 crate 扫描主目录；SoA/名字 arena；查询/表格基准；真实 RSS 与快照复制；macOS 元数据枚举对比 | 4–6 |
| M0b 系统集成 | Windows 监听溢出/安全弹出；TCC/签名；原生对话框层级与焦点；IME 事件序列；搜索页集合行为 | 3–5 |
| M1 骨架与接入 | 模块内分层、嵌套核心、设置单写、默认关闭开关/热键/入口、统一导航、退出/on_exit、HostServices 注入、权限引导骨架 | 5–6 |
| M2 索引闭环 | 元数据扫描、根身份/重叠、排除、快照、查询分页、基本结果操作，两平台均可用 | 5–7 |
| M3 双平台实时更新 | FSEvents/RDCW、检查点/冷启动校验、热插拔/句柄释放、事件风暴、代际一致性 | 6–8 |
| M4 界面与设置 | 表格、列排序、历史、设置差异应用、图标、Floating/Pinned、对话框及拖入、无障碍标签 | 4–6 |
| M5 联合验收 | 两平台真实设备、中文输入法、权限升级、云占位、恢复/关停/性能回归 | 4–6 |

基础合计 31–44 人天（比第二版多出退出/宿主能力/对话框集成约 2–3 天），另留 30%–50% 缓冲，排期约 40–66 人天。M0 输出应明确通过项、失败项及修订决定；Windows 不再拖到 M5 才开始。

v2 的 Normal/独立视口、USN、拼音、内容搜索等不在本次估算内。代码落地时逐阶段验证，本次仅完成方案修订。

若进度吃紧，最先可以后置的是：保存搜索、历史、创建时间列、图标。索引 + 查询 + 监听 + 打开/显示是主干，其余是体验项；压缩测试和验收标准不是可选项。

## 12. 已定决策与剩余验证

| 事项 | 决策 |
| --- | --- |
| 功能归属 | 搜索业务全部在 devtools-search；原生文件夹选择、系统模态保护、窗口拖动由宿主平台实现，trait 在模块 |
| 主模块设置 | 仅搜索开关和快捷键；开关默认 false |
| 搜索设置 | 独立模块页面、独立 JSON、Service 单写 |
| 退出 | 托盘/更新重启走正常关闭；`on_exit` 有界保存；崩溃不保证 |
| 默认根/排除 | 主目录；node_modules/target/DerivedData/.git 默认排除；macOS 默认排除整个 `Library/`，Windows 排除 Temp/Packages/已知缓存；内置自目录不可删 |
| v1 窗口 | Floating / Pinned；搜索页切 `MoveToActiveSpace` + 不透明底；Normal 与独立视口归 v2 |
| 索引启动 | 默认首次打开；可在模块设置选择后台预热，但权限引导优先 |
| 查询语法 | 移植适用解析部分；入口文本按字面；公开支持集合以 §7.8 为准 |
| 热插拔 | v1 两平台必做；Windows query-remove 必做 |
| 并发 | 每根 RwLock、专用池、短批提交；不做整个索引的写时复制 |
| 行批次 | 绑定 session/query/root/epoch + 逐行状态；revision 只失效结果集与排序 |
| 节点状态 | `deleted` / `excluded` / `pending_remove` 分开；ID 不复用 |
| 元数据 | 扫描预取，查询不 stat，创建时间 btime |
| 网络根 | v1 明确拒绝；不留下不可取消网络扫描的口子 |
| 配置版本 | 从 1 开始；没有历史配置迁移 |
| 托盘入口 | 常在，关闭时禁用并显示"（未启用）" |

仍需 M0 验证而非再次拍板的事项：macOS 枚举实现、真实内存/延迟预算、原生对话框的层级与焦点恢复、设备消息/句柄释放时限、快捷键与输入法冲突、自定义表格成本、语法移植与许可核验、`MoveToActiveSpace` 在 Pinned 下的实际手感。生产签名配置是发布前置条件，不能伪装成已经满足。

## 13. 参考与复用

- [DevTools Hub 基线](https://github.com/zvae/devtools_hub/tree/679ab298f4b7d564616015c6daf435a17e40003e)：核对了 Cargo、设置、热键、页面生命周期、controller、快捷面板入口、托盘退出、保存对话框的层级处理以及 CI/发布配置。
- [EverythingMac](https://github.com/seedds/EverythingMac)：参考五列表格、名字驻留、多卷与查询解析思路。
- [MacEverything](https://github.com/joshua-wu/MacEverything)：内容/trigram/WAL 等仅作为后续研究，本期不引入其服务接口。

外部参考项目本次未构建或运行。原方案记载许可证为 MIT；正式移植前必须核对所选提交及子目录许可证，保留版权声明和来源，在主 README 添加致谢链接。本文不以尚未核验的解析器"零依赖"等特征作为确定事实。

## 附录 A：2026-10-30 第一轮评审逐项处理

> 本表保留第一轮的处理记录；第二版、第三版的进一步修订以正文和附录 B 为准。

| 编号 | 处理结果 | 对应章节 |
| --- | --- | --- |
| P0-1 | 首扫引导、系统可能弹授权、拒绝目录可见、签名发布条件 | 5.3、8.2、10.2 |
| P0-2 | 模块内设备线程，取消扫描/关闭句柄，双平台安全弹出验收 | 7.5、8.2–8.3、10.2 |
| P0-3 | Windows 旧快照可查但标校验，全量重扫回放后替换；USN v2 | 1.2、7.7 |
| P0-4 | 卷身份+相对路径根键，根/忽略目录同模型 | 6.1、7.4–7.6 |
| P0-5 | 拒绝重复/嵌套根，挂载时复验 | 7.5 |
| P0-6 | 设置/缓存根为不可删除排除项，所有扫描路径一致适用 | 7.4 |
| P1-7 | policy_changed 通知宿主更新；Normal 不在 v1 | 4.1、4.4 |
| P1-8 | 统一切页/隐藏/退出，覆盖 controller 直接窗口操作 | 4.2、4.4 |
| P1-9 | FolderPicker、原生对话框依赖、失焦保护、拖入 | 3.4、5.4、8.1 |
| P1-10 | 只保留 Floating/Pinned；可见未聚焦时热键前置 | 4.4、5.8 |
| P1-11 | 墓碑不复用、根代际/版本、LoadRows 校验、防抖重查 | 7.1、7.7、7.9 |
| P1-12 | 每根 RwLock、专用池、小批提交、后台优先级与介质并发 | 7.3、7.10 |
| P1-13 | 元数据扫描预取，Windows 缓存元数据，macOS M0 对比 | 7.3、10.3 |
| P1-14 | last_event_id 与节点同根快照保存，代际文件+manifest 提交 | 7.6 |
| P1-15 | NFC 必选，明确 Unicode lowercase 与完整 folding 差异 | 3.4、7.1 |
| P2-16 | 接受独立核心编译边界，但按用户要求嵌套于 devtools-search | 3.1–3.3 |
| P2-17 | Service 持有唯一设置，core 仅收 IndexConfig | 2、3.2、6 |
| P2-18 | new 不扫描，warm_up 处理 lazy_start，主开关与授权优先 | 4.1、4.4 |
| P2-19 | SoA+连续名字缓冲区，M0 真实 RSS 和峰值 | 7.1–7.2 |
| P2-20 | ctime 改为 btime，未知创建时间不伪造 | 7.1 |
| P2-21 | u32 上限修正为约 42.9 亿 | 7.1 |
| P2-22 | 路径尾段→parent 链，infolder 使用有效 DFS 区间或回退 | 7.8 |
| P2-23 | 新增排除剪枝，删除排除补扫受影响范围 | 6.3 |
| P2-24 | 增补平台缓存默认排除，保留用户可调整范围 | 7.4 |
| P2-25 | 快照本地缓存；核实当前 Windows 已为 LOCALAPPDATA，不虚构迁移 | 4.3 |
| P2-26 | IconKey+不可任意构造路径凭据，占位项通用图标 | 8.1 |
| P2-27 | v1 SystemActions 不含 open_terminal | 1.2、8.1 |
| P2-28 | Enter 按焦点区分提交/打开/设置控件 | 5.2 |
| P2-29 | 仅提交或打开时记历史，支持关闭和清空 | 5.2、6.1 |
| P2-30 | 承认 TableBuilder 能力，M0 对比，自定义预算 2–3 人天 | 3.4、11 |
| P2-31 | 补齐 pause/persist_index、线程方案、开发依赖及对应 UI/JSON | 3.4、5.5、6、7.10 |
| P2-32 | 拼音和首字母明确 v2 | 1.2 |

## 附录 B：2026-10-30 第二轮评审逐项处理

| 编号 | 处理结果 | 对应章节 |
| --- | --- | --- |
| 退出路径 | 托盘/更新重启改正常关闭；`on_exit`；`shutdown` 两种语义；验收补退出保存 | 4.1、4.2、4.4、10.2 |
| 原生对话框 | FolderPicker/SystemModalGuard 的 trait 在模块、实现在宿主；去掉 `suppress_hide_on_blur`；模态期间不强制关闭 | 0.2、3.1、4.1、5.3–5.4、8 |
| 行批次 revision | 行只校验 session/query/root/epoch + 行状态；revision 只失效结果集 | 7.1、7.7、7.9 |
| 墓碑三义 | 拆成 deleted / excluded / pending_remove，定义 LoadRows 行为 | 6.3、7.1、7.7 |
| 高亮映射 | 只对可见行按需重算，按字符区间；不计入常驻内存 | 7.8、7.2 |
| 名字预算 | 三份文本共享 arena + 标志；25MB 作为 M0 实测目标 | 7.1–7.2、10.3 |
| 默认排除 | macOS 默认排除整个 `Library/`；Windows 补 Temp/Packages/缓存；M0 统计 | 6.1、7.4、10.3 |
| 窗口行为 | 搜索页 `MoveToActiveSpace` + 不透明底 + 顶栏拖动；Pinned 不跟随桌面 | 1.1、5.1、5.8、8.2 |
| 托盘显隐 | 改为常在 + 禁用态（muda 无 set_visible） | 4.2、12 |
| 入口收口 | controller 只 post 事件；列出 9 处 page 赋值；统一 `set_page` | 4.2、4.4 |
| 快捷面板 | FileSearch 始终可用；改 `actions_for`/`has_match`；文本按字面 | 4.2、4.5、5.2 |
| 接入清单 | 补 hotkey_errors/ALL 长度、draw inner_margin/toast、handle_focus、on_exit | 4.2 |
| 缓存与配置 | 实例缓存 GC；配置从 version 1；RootLocation 归 core | 4.3、6.1–6.2、3.1 |
| 其它 P2 | 挂载点判断、图标策略、等宽数字、infolder 越界、size/type 表、压缩、无障碍、M0 拆分 | 5.1、5.7、7.1、7.3、7.8、11 |
