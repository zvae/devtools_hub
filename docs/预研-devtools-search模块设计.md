# devtools-search 文件搜索模块设计方案

> 状态：预研方案，待评审  
> 日期：2026-10-29  
> 范围：macOS / Windows 本地文件搜索；界面、搜索设置、索引、监听全部在 `devtools-search`  
> 参考项目：EverythingMac（界面、索引与多卷思路）、MacEverything（trigram/内容索引思路，本期不采用）  
> 参考仓库：  
> - EverythingMac：<https://github.com/seedds/EverythingMac>（本地参考目录 `temp_reference/EverythingMac-main`）  
> - MacEverything：<https://github.com/joshua-wu/MacEverything>（本地参考目录 `temp_reference/MacEverything-main`）  
> 说明：两个参考项目只做了源码阅读，没有构建或运行，如果后续做了代码参考引用，需要在主readme中添加致谢链接。

## 0. 方案摘要

- 新建 `crates/devtools-search`，内部按 `ui / core / platform / settings / ports` 分层；主模块只依赖它的公开 API（`SearchModule` / `SearchOptions` / `SearchUiOutput` / `WindowPolicy` / `SearchWindowMode`）。
- 界面按 EverythingMac 的五列表格实现，但用 egui 重写；UI、搜索设置、索引、监听、图标全部在模块内。
- 主模块只增加页面入口、托盘/快捷面板入口、全局快捷键和一个 `search_hotkey` 字段；搜索设置写入 `data_dir/search/settings.json`。
- 排除规则从第一版支持 `node_modules`、`node_modules/`、`*.log`、`**/build/**` 等 basename/路径规则，扫描与增量事件都生效。
- 窗口模式按页面切换：搜索页支持 `floating` / `pinned` / `normal`，其他页面行为不变；只有需要 Dock/任务栏/双窗口时才用独立视口。
- 索引从一开始就是多根结构；外置盘必须显式添加为根，挂载时扫描、卸载时移除，不自动索引所有卷。
- 默认只索引主目录、懒加载；内容搜索、MCP/HTTP、拖出、Quick Look 不在本期范围。

## 1. 目标与非目标

### 1.1 目标

- 在 DevTools Hub 中新增“文件搜索”功能，界面按 EverythingMac 的数据表形态实现：
  顶部搜索栏 + 文件夹范围 + 结果表格 + 底部状态栏。
- 功能实现全部收敛在 `crates/devtools-search`：包括界面、索引、查询、监听、搜索设置。
- 主模块 `devtools-hub` 只增加：
  - 新页面入口；
  - 全局快捷键的注册与分派；
  - 菜单栏 / 快捷面板 / 右键菜单入口。
- 搜索设置与主设置完全分开，保存在 `data_dir/search/settings.json`，不写进主 `settings.json`。
- 支持按名字/glob 排除目录，例如 `node_modules`、`target`、`*.log`。
- 支持多个索引根，包括外置硬盘的挂载 / 卸载；默认只索引用户显式添加的根。
- 搜索与索引都在后台完成，UI 线程不做磁盘遍历和全量匹配。

### 1.2 非目标（本期不做）

- 文件内容搜索（`infile:` / `content:`）和 trigram 内容索引。
- HTTP API、MCP、CLI daemon。
- EverythingMac 的 Events 标签、Quick Look、拖出到 Finder / VS Code、重命名 / 废纸篓 / 终端打开。
- 云盘、网络卷的自动索引（网络卷默认跳过，用户显式添加时再考虑）。
- 与 Spotlight / `mdfind` 集成。
- 多窗口并存（搜索窗口和剪贴板面板同时显示）不在本期；搜索页的普通窗口模式由 5.8 的页级窗口模式支持，不需要重写其他页面。

## 2. 设计原则

1. **单向依赖**：`ui → core`，`platform → core/ports`，`core → ports`，任何层都不反向依赖 UI。
2. **主模块零搜索逻辑**：主模块不出现索引、查询、根目录、排除规则、排序列宽等类型。
3. **搜索设置自包含**：除全局快捷键外，所有搜索设置由 `devtools-search` 读写。
4. **平台代码隔离**：`cfg(target_os)` 只出现在 `platform/`，`core/` 中不出现 `objc2`、`windows-sys`、`egui`。
5. **先懒加载，后常驻**：首次打开搜索页时才加载快照 / 建立索引，避免拖慢 DevTools Hub 启动和常驻内存。
6. **默认安全**：只索引用户添加的根；不自动索引所有卷；网络卷默认跳过；索引数据只保存在本地。
7. **可验证**：分层边界用脚本检查，核心逻辑用临时目录测试，不依赖真实全盘。

## 3. 总体架构

### 3.1 依赖方向

```
                    ┌──────────────────────────────┐
                    │        devtools-search       │
                    │                              │
  devtools-hub ───▶ │  lib.rs（组合根 / 唯一公开 API）│
                    │    ├── ui/          egui     │
                    │    ├── settings/    serde    │
                    │    ├── core/        纯 Rust  │
                    │    ├── ports.rs     接口定义 │
                    │    └── platform/    objc2 /  │
                    │                     windows  │
                    └──────────────────────────────┘
```

- `devtools-hub` 只依赖 `devtools-search` 的公开 API。
- `devtools-search` 不依赖 `devtools-hub`、`devtools-clipboard`、`devtools-json`。
- `core` 不依赖 `egui` 和平台 crate；通过 `ports` 里的 trait 访问文件系统、监听器、图标和系统动作。
- `ui` 不直接调用 `objc2` / `windows-sys`；只使用 `ports` 暴露的接口。
- `settings` 只包含数据结构和读写，不依赖 `ui`。

### 3.2 目录结构

```
crates/devtools-search/
├── Cargo.toml
├── src/
│   ├── lib.rs                 # 组合根：构造 platform + core + settings + ui，只公开 SearchModule
│   ├── ports.rs               # 跨层接口与纯数据类型（无 egui / 无平台依赖）
│   ├── settings/
│   │   ├── mod.rs             # SearchSettings 的读写、默认值、迁移、校验
│   │   ├── model.rs           # 根目录、排除规则、UI 偏好、历史、保存搜索
│   │   └── store.rs           # search/settings.json 原子读写
│   ├── core/
│   │   ├── mod.rs
│   │   ├── model.rs           # Node、NameId、RootId、元数据
│   │   ├── index.rs           # 节点表、名字驻留、子节点、路径链
│   │   ├── query/
│   │   │   ├── mod.rs         # 统一的 query AST 与执行入口
│   │   │   ├── parse.rs       # 子集解析器；可选移植 everything-mac-syntax
│   │   │   ├── eval.rs        # 词项、布尔、过滤器求值
│   │   │   └── sort.rs        # 路径序/列排序
│   │   ├── scan/
│   │   │   ├── mod.rs         # 遍历调度、进度、取消
│   │   │   └── exclude.rs     # 排除规则匹配
│   │   ├── snapshot.rs        # 快照格式、校验和、原子保存
│   │   ├── service.rs         # 命令/事件、代际取消、分页、线程编排
│   │   └── volumes.rs         # 多根、挂载/卸载状态机
│   ├── platform/
│   │   ├── mod.rs             # cfg 选择 macos / windows
│   │   ├── macos/
│   │   │   ├── fs.rs          # 遍历、元数据
│   │   │   ├── watch.rs       # FSEvents、Mount/Unmount
│   │   │   ├── volumes.rs     # getfsstat、卷标识
│   │   │   ├── icons.rs       # 文件图标取 RGBA
│   │   │   └── actions.rs     # 打开、在访达中显示
│   │   └── windows/
│   │       ├── fs.rs
│   │       ├── watch.rs       # ReadDirectoryChangesW
│   │       ├── volumes.rs     # 盘符/卷 GUID、设备变化
│   │       ├── icons.rs       # SHGetFileInfoW
│   │       └── actions.rs     # 打开、在资源管理器中显示
│   └── ui/
│       ├── mod.rs             # SearchView：组合各子视图
│       ├── header.rs          # Aa、搜索库、搜索框、文件夹范围
│       ├── results_table.rs   # 虚拟化表格、列宽、排序、选择
│       ├── status_bar.rs      # 状态、计数、耗时
│       ├── settings_panel.rs  # 搜索设置页（模块内）
│       ├── history.rs         # 最近搜索、保存搜索
│       ├── icons.rs           # 图标异步加载与纹理缓存
│       ├── text.rs            # 中间截断、高亮
│       └── theme.rs           # 从 egui 全局样式派生，不依赖 hub
└── tests/
    ├── query.rs
    ├── exclusions.rs
    ├── snapshot.rs
    ├── volumes.rs
    └── layering.rs            # 3.4 的跨平台分层检查
```

### 3.3 分层职责

| 层 | 职责 | 可以依赖 | 禁止 |
| --- | --- | --- | --- |
| `lib.rs` | 组装、启动/关闭、公开 API | 所有内部层 | 放业务逻辑 |
| `ui` | 绘制、输入、选择、列宽、设置表单 | `core` 的公开类型、`settings`、`ports`、egui | 读写索引、调用平台 API、访问 `core::index` 内部结构 |
| `core` | 索引、扫描、查询、排序、快照、服务状态 | `ports`、`settings` 数据类型、`std` | 出现 `egui`、`objc2`、`windows-sys`、文件对话框 |
| `platform` | 遍历、监听、卷枚举、图标、系统动作 | `ports`、`core` 的纯数据类型 | 出现 `egui`、UI 文案、设置读写 |
| `settings` | 搜索配置的默认值、校验、JSON 读写 | `std`、serde | 出现 UI 或索引 |
| `ports` | trait 与纯数据 | 无 | 依赖任何内部层 |

### 3.4 边界检查

用 `crates/devtools-search/tests/layering.rs` 做检查，不用 shell 脚本：项目 CI 只有 `windows-latest` / `macos-15`，不能假设有 `rg`；测试跟随 `cargo test --workspace` 在两个平台运行。

- 遍历 `src/core`、`src/ui`、`src/platform`、`src/settings` 下的 `.rs` 文件。
- 先去掉 `//` 与 `/* ... */` 注释，再匹配，避免注释里的示例路径误报。
- 使用词边界匹配：`\begui::`、`\bobjc2`、`\bwindows_sys`、`\bcrate::platform\b`、`\bcrate::ui\b`。
- 规则：
  - `core` 禁止 `egui::`、`objc2`、`windows_sys`、`crate::ui`、`crate::platform`；
  - `platform` 禁止 `egui::`、`crate::ui`；
  - `settings` 禁止 `egui::`、`objc2`、`windows_sys`；
  - `ui` 禁止 `objc2`、`windows_sys`、`crate::platform`。
- 字符串字面量里出现这些词仍可能误报；如果出现，用 `#[allow]` 不行，应该在测试里排除已知白名单，并保持白名单最小。
- 可选保留 `scripts/check-search-layers.sh` 作为本地快速检查，但 CI 以 `layering.rs` 为准。

`core::index` 等内部模块使用 `pub(crate)` 可见性，`ui` 只能通过 `core` 暴露的只读查询结果访问数据。

### 3.5 新增依赖

遵循根 `Cargo.toml` 的“依赖克制”约定，新增依赖及理由如下：

| 依赖 | 范围 | 理由 |
| --- | --- | --- |
| `egui` | 全平台 | 仅 `ui` 层使用；版本跟随 workspace |
| `serde` / `serde_json` | 全平台 | 搜索设置读写 |
| `tracing` | 全平台 | 日志，沿用主模块 subscriber |
| `zstd` | 全平台 | 索引快照压缩，workspace 已有 |
| `sha2` | 全平台 | 快照校验，workspace 已有 |
| `anyhow` | 全平台 | 内部错误上下文，workspace 已有 |
| `globset` | 全平台 | 排除规则的 `*` / `?` / `**` 匹配 |
| `rayon` | 全平台 | 并行遍历与并行名字匹配 |
| `postcard` | 全平台 | 紧凑快照序列化，替代 JSON |
| `thin-vec` | 全平台 | 目录子节点列表，降低每节点开销 |
| `regex` | 全平台 | `regex:` 查询过滤器 |
| `unicode-normalization` | 可选 | macOS 文件名 NFC/NFD 归一化后再比较 |
| `objc2` / `objc2-app-kit` / `objc2-foundation` | macOS | 文件图标、系统打开/显示、NSWorkspace 通知 |
| `objc2-core-services` / `objc2-core-foundation` / `dispatch2` | macOS | FSEvents 监听 |
| `libc` | macOS | `getfsstat` 等卷枚举 |
| `windows-sys` | Windows | 文件系统、`ReadDirectoryChangesW`、盘符/卷、图标、`ShellExecuteW` |

不引入：

- `egui_extras` / `egui_table`：表格自己实现，保持样式与现有面板一致。
- `notify` / `walkdir` / `ignore`：监听和遍历需要精确控制排除、卷、事件分类和取消，自己封装平台 API。
- 任何 HTTP / MCP / 内容索引库。

## 4. 对外 API 与主模块集成

### 4.1 唯一公开 API

```rust
// crates/devtools-search/src/lib.rs
pub struct SearchModule {
    // 内部：SearchView + Service + SettingsStore
}

#[derive(Clone, Debug, Default)]
pub struct SearchOptions {
    /// 打开时预填的查询，空表示保留上次输入。
    pub query: Option<String>,
    /// 打开时预填的文件夹范围。
    pub folder: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchWindowMode {
    /// 现有浮层：置顶、失焦隐藏。
    Floating,
    /// 固定浮层：置顶、不随失焦隐藏。
    Pinned,
    /// 普通窗口：不置顶、可被其他应用覆盖；只作用于搜索页。
    Normal,
}

#[derive(Clone, Copy, Debug)]
pub struct WindowPolicy {
    pub size: [f32; 2],
    pub min_size: [f32; 2],
    pub mode: SearchWindowMode,
    pub hide_on_blur: bool,
    /// Normal 模式下是否显示标题栏；Floating / Pinned 模式忽略。
    pub decorations: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SearchUiOutput {
    /// 用户按 Esc / 关闭键，请求宿主隐藏窗口。
    pub request_hide: bool,
}

impl SearchModule {
    /// 只加载 `data_dir/search` 下的搜索设置和构造视图，不启动扫描线程；
    /// 首次 on_shown/ui 时才启动索引。
    pub fn new(data_dir: &Path, ctx: egui::Context) -> Self;

    /// 绘制搜索页。宿主只负责提供 Ui（窗口、背景、边距由宿主决定）。
    pub fn ui(&mut self, ui: &mut egui::Ui) -> SearchUiOutput;

    /// 窗口显示时调用：聚焦搜索框、启动/加载索引、刷新状态。
    pub fn on_shown(&mut self, options: SearchOptions);

    /// 窗口隐藏时调用：保存 UI 偏好；索引是否继续监听由搜索设置决定。
    pub fn on_hidden(&mut self);

    /// 当前窗口策略，宿主在显示窗口前读取。
    pub fn window_policy(&self) -> WindowPolicy;

    /// 宿主在窗口收起时把实际大小交回给搜索模块保存。
    pub fn remember_size(&mut self, size: [f32; 2]);

    /// 进程退出时调用：停止监听、保存快照。
    pub fn shutdown(&mut self);
}
```

公开 API 只有上面这些。`SearchModule` 内部可以拆分 `SearchView`、`ServiceHandle`、`SettingsStore`，但不暴露给主模块。

### 4.2 主模块改动清单

主模块只改“入口 + 快捷键”，不出现任何搜索设置字段。

| 文件 | 改动 | 说明 |
| --- | --- | --- |
| 根 `Cargo.toml` | `members` 增加 `crates/devtools-search`；`workspace.dependencies` 增加 `devtools-search = { path = "crates/devtools-search" }` | 工作区接线 |
| `crates/devtools-hub/Cargo.toml` | 增加 `devtools-search.workspace = true` | 唯一新增依赖 |
| `crates/devtools-hub/src/ui/mod.rs` | `Page` 增加 `Search`；`HubApp` 增加 `search: SearchModule`；构造时 `SearchModule::new(&data_dir, cc.egui_ctx.clone())`；`draw` 分支调用 `self.search.ui(ui)`；`on_shown` 调用 `search.on_shown`；`hide_panel` 分支调用 `search.remember_size` / `on_hidden`；`handle_focus` 使用 `window_policy().hide_on_blur`；`show_search()` 按 `policy.mode` 应用/恢复窗口模式 | 窗口与页面宿主 |
| `crates/devtools-hub/src/platform/{macos,windows}/panel.rs` | 增加 `set_mode(WindowMode)`：只切换窗口层级 / 集合行为 / 激活策略；其他页面仍默认走现有 Panel 行为 | 仅窗口模式接线，不重写窗口 |
| `crates/devtools-hub/src/hotkey.rs` | `HotkeyAction` 增加 `Search`；`ALL` 增加一项；回调映射到 `AppEvent::Search` | 主模块负责注册快捷键 |
| `crates/devtools-hub/src/settings.rs` | 只增加 `search_hotkey: String` 与默认值；`hotkey()` / `hotkey_mut()` 增加分支 | 这是主设置中唯一与搜索有关的字段，仅用于全局快捷键 |
| `crates/devtools-hub/src/shared.rs` | `AppEvent` 增加 `Search` | 热键/托盘/快捷面板统一入口 |
| `crates/devtools-hub/src/tray.rs` | 菜单增加“文件搜索”，显示快捷键提示，点击 post `AppEvent::Search` | 入口 |
| `crates/devtools-hub/src/tools/mod.rs` | `QuickAction` 增加 `FileSearch`，始终可用；`quick_panel.rs` 分派到 `show_search()` | 入口 |
| `crates/devtools-hub/src/ui/settings_page/general.rs` | 在快捷键分组增加 `HotkeyAction::Search` 一行 | 只配置快捷键 |
| `crates/devtools-hub/src/ui/mod.rs` (`on_exit`) | 调用 `self.search.shutdown()` | 保存快照、停止监听 |

主模块禁止出现：

- `roots`、`exclude_patterns`、`debounce`、`sort`、`columns`、`index_path` 等搜索设置字段；
- 搜索表格、搜索设置面板、文件图标加载、索引/查询类型；
- 对 `devtools_search` 内部模块的引用，只允许 `SearchModule`、`SearchOptions`、`SearchUiOutput`、`WindowPolicy`、`SearchWindowMode`。

窗口最小尺寸不新增平台 API：主模块在切到 `Page::Search` 时发送 `egui::ViewportCommand::MinInnerSize(policy.min_size)`，切回其他页面时恢复现有最小值 `[240, 120]`；`inner_margin`、`toast` 的页面分支也补上 `Page::Search`。这些都只是宿主接线，不含搜索逻辑。

按此方案，主模块预计净增约 150–200 行（页面、入口、快捷键），其余实现全部在 `devtools-search`；验收时用 `git diff --stat` 检查主模块改动范围。

### 4.3 搜索设置与主设置的边界

- `data_dir/settings.json`（主模块）：剪贴板、截屏、托盘、`search_hotkey`。
- `data_dir/search/settings.json`（搜索模块）：根目录、排除规则、索引开关、搜索延迟、列宽、排序、窗口大小、隐藏策略、搜索历史、保存搜索。
- `data_dir/search/index/`（搜索模块）：`manifest.json` + 每根一个快照文件。
- 主模块的 `Settings::load/save` 不读写 `search/settings.json`；搜索模块自己负责原子写入与损坏恢复。

快捷键归属说明：

- 方案 A（本文默认）：`search_hotkey` 放在主设置里。原因是全局热键服务、录音 UI、注册失败提示都在主模块；“配置快捷键”本来就是主模块的职责，搜索模块不参与。
- 方案 B（可选）：如果评审认为快捷键也应属于搜索设置，则 `SearchModule` 额外暴露 `hotkey()` / `set_hotkey(spec)`，主模块启动和设置变化时读取后注册，主 `Settings` 不新增字段。两种方案对搜索模块内部没有影响。

无论选哪种，搜索页的窗口大小、列宽、排序、根目录、排除规则等都不进入主设置。

### 4.4 打开与关闭时序

```
热键 / 托盘 / 快捷面板
        │
        ▼
主模块 AppEvent::Search
        │
        ├─ 已在搜索页且窗口可见 → 隐藏
        └─ 否则：
             page = Page::Search
             search.on_shown(SearchOptions::default())
             panel::show(ScreenCenter, search.window_policy().size)
                     │
                     ▼
             search.ui(ui)
               ├─ 用户输入 → Service 搜索 → 完成事件 → 重绘
               ├─ 用户按 Esc → SearchUiOutput { request_hide: true }
               └─ 用户点设置 → 模块内设置页
                     │
                     ▼
主模块 hide_panel()
  ├─ search.remember_size(实际内容区大小)
  ├─ search.on_hidden()
  └─ panel::hide()
```

`show_search()` 中如果当前页面就是搜索页且窗口可见，则执行收起，保持与现有 `toggle_panel` 一致。普通窗口模式不要求改其他页面：宿主只增加一个 `apply_search_window_mode(mode)`，进入搜索页时切换，离开时恢复为默认浮层；剪贴板、JSON 编辑器、OCR 仍走原有代码。主模块接线大致如下（只做窗口与事件，不碰搜索内部）：

```rust
fn show_search(&mut self) {
    if self.page == Page::Search && native_panel::is_active() {
        self.hide_panel(false);
        return;
    }
    self.startup_hide_pending = false;
    self.cancel_hotkey_recording();
    self.page = Page::Search;
    self.search.on_shown(SearchOptions::default());
    let policy = self.search.window_policy();
    self.ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(
        egui::vec2(policy.min_size[0], policy.min_size[1]),
    ));
    self.apply_search_window_mode(policy.mode);
    native_panel::show(Anchor::Placement(Placement::ScreenCenter), policy.size);
}

// draw() 中
Page::Search => {
    hide_after_draw = self.search.ui(ui).request_hide;
}
```

`hide_panel()` 中：

```rust
if self.page == Page::Search {
    self.search.remember_size(size); // 由搜索模块写入自己的 settings.json
    self.search.on_hidden();
    self.apply_search_window_mode(SearchWindowMode::Floating);
}
```

`apply_search_window_mode` 的实现放在主模块 `ui/mod.rs`，只是根据 `mode` 调用 `platform::panel::set_mode`；`set_mode` 在 macOS 上切换窗口层级、集合行为和失焦策略，在 Windows 上切换置顶/普通层级。其他页面从不调用它，默认行为保持不变。切回其他页面时把最小尺寸恢复为 `[240.0, 120.0]`。

### 4.5 快捷键默认值

主模块沿用现有 `global-hotkey` 服务：

- macOS：`Cmd+Shift+Space`（与 EverythingMac 默认一致，用户可改；与已安装的 EverythingMac 冲突时注册会失败并在设置页显示原因）。
- Windows：`Alt+Shift+F`（与现有 `Alt+Shift+V` 的修饰键习惯一致）。

主设置页只显示“文件搜索快捷键”一行，不出现搜索设置分组；搜索行为设置只能在搜索页的齿轮里打开。

## 5. 界面设计

### 5.1 布局

按 EverythingMac 的内容结构，但使用 DevTools Hub 的字体、圆角与玻璃背景。

```
┌────────────────────────────────────────────────────────────┐
│ [Aa] [历史] [ 搜索文件或 ext:pdf ...              ] [文件夹×] [⚙] │
│ 权限/索引提示横幅（仅在需要时出现）                            │
├────────────────────────────────────────────────────────────┤
│ 文件名                路径                       大小   修改时间 │
│ 📄 report.md          ~/work/docs                 4.2 KB  10-28  │
│ 📁 src                ~/work/project                      10-27  │
│ ...                                                        │
├────────────────────────────────────────────────────────────┤
│ ● 就绪 · 索引 1,234,567 项 · 38 个结果 · 6 ms        [固定]  │
└────────────────────────────────────────────────────────────┘
```

- 默认窗口 1100×700，最小 800×480；大小由搜索设置保存。
- 顶部栏内边距 10，搜索框高度约 34。
- 结果表格占满剩余空间；底部状态栏 32 高。
- 搜索页不绘制窗口背景和圆角，由主模块现有 `draw` 绘制；搜索模块只画内容。
- 默认窗口模式 `floating`（与现有浮层一致），顶栏提供固定按钮切换到 `pinned`；`normal` 在设置里选择。`window_policy().hide_on_blur` 只在 `floating` 模式为 true。

### 5.2 结果表格

- 默认列：文件名 260、路径 450、大小 100、修改时间 155、创建时间 155（逻辑像素，按实际窗口缩放）。
- 行高 24，表头高 26；隔行底色；悬停高亮；选中行使用 `ui.visuals().selection`。
- 文件名前 16px 文件图标；路径列显示父目录；大小、日期右对齐等宽数字；单行中间截断。
- 列宽可拖动调整，列宽、排序列、排序方向保存到搜索设置。
- 点击表头在升序 / 降序 / 默认（路径升序）之间循环；表头显示箭头。
- 多选：单击、Shift 连选、Cmd/Ctrl 点选；`Cmd/Ctrl+A` 全选；上下键移动；从第一行向上返回搜索框。
- 双击打开；右键菜单：打开、在访达/资源管理器中显示、复制路径、复制文件名。
- 结果超过可见区域时只渲染可见行和少量 overscan；不创建全部行控件。
- 状态栏显示：生命周期、索引条目数、结果数、查询耗时、选中数量、固定按钮。

| 操作 | 动作 |
| --- | --- |
| `Cmd/Ctrl+F` | 聚焦搜索框 |
| `Enter` | 立即提交搜索 |
| `Down` / `Up` | 从搜索框进入结果 / 从第一行回到搜索框 |
| `Cmd/Ctrl+O` | 打开选中文件 |
| `Cmd/Ctrl+R` | 在访达 / 资源管理器中显示 |
| `Cmd/Ctrl+Shift+C` | 复制选中路径 |
| `Cmd/Ctrl+A` | 全选结果 |
| `Esc` | 查询非空时先清空；查询为空时请求宿主隐藏 |

### 5.3 空态与提示

- 首次打开且未建索引：显示“正在建立索引…”和已完成数量；仍可打开设置。
- 无结果：显示“没有匹配项”，附当前查询。
- 结果超过 `max_results`：状态栏显示“显示前 200,000 项 / 共 N 项，继续输入可收窄”，不阻塞滚动和打开文件。
- 快照损坏：提示“索引已损坏，正在重建”，自动重建。
- 根目录离线：该根显示“离线”，结果中移除该根；提示横幅不阻塞搜索。
- macOS 无完全磁盘访问权限：顶部横幅说明并给“打开系统设置”按钮；不自动弹窗。

### 5.4 搜索栏与搜索库

- `Aa`：大小写敏感开关，默认值来自搜索设置。
- 历史按钮：弹出最近搜索（最多 100 条）与保存搜索；保存搜索包含查询、文件夹范围、Aa 状态。
- 搜索框：单行输入，支持输入法；Enter 立即搜索；输入按 `debounce_ms` 延迟搜索（默认 100ms）；IME 组合期间暂停查询（见 5.9）。
- 文件夹范围：显示为可删除的胶囊；点“文件夹”选择目录；为空表示全部根。
- 设置齿轮：在当前页面内切换到搜索设置面板，不打开主设置页。

### 5.5 搜索设置面板（模块内）

设置面板也由 `devtools-search` 绘制，布局为单页分组：

1. **索引根**：根目录列表（路径、卷标识、状态、条目数），添加/移除目录，启用/停用；显示“添加外置硬盘”的说明。
2. **排除规则**：多行文本，每行一条：
   - `node_modules`：任意层级、名字匹配；
   - `node_modules/`：只匹配目录；
   - `*.log`：名字通配；
   - `**/build/**`：路径匹配并剪枝；
   - 绝对路径忽略项单独一组。
3. **索引行为**：启动时加载 / 首次打开时加载、是否监听实时更新、是否索引 `.app` 内部、是否允许网络卷。
4. **搜索行为**：默认大小写、延迟、空查询是否显示全部、结果上限（默认 200,000，0 表示不限）、高亮开关。
5. **索引状态**：节点数、名字数、快照大小、上次保存、上次扫描耗时、重建/保存/清除按钮。
6. **界面**：窗口大小、失焦隐藏、固定窗口、重置列宽/排序。

索引相关设置修改后不即时生效：底部按钮“应用并重建”，右侧“还原”。界面偏好即时保存。

### 5.6 主题与组件

- 搜索 UI 不依赖 `devtools-hub::ui::style::Palette`；从 `ui.visuals()` 取颜色：
  - 强调色用 `ui.visuals().hyperlink_color`；
  - 弱文本用 `ui.visuals().weak_text_color()`；
  - 隔行底色用 `faint_bg_color`；
  - 选中行用 `selection.bg_fill`；
  - 警告用 `warn_fg_color`。
- 字体、字号、圆角由主模块安装的 egui 全局样式提供；搜索模块不重复安装字体。
- 在独立测试中，搜索模块使用 egui 默认样式即可显示。

### 5.7 egui 实现约定

- 表格用自定义虚拟化组件（参考现有 `panel/cards.rs` 的 `ScrollArea::show_viewport`），不引入 `egui_extras`。
- 表头固定；列宽拖拽命中区 ±3px；使用 `CursorIcon::ResizeHorizontal`。
- 中间截断用字体布局测量后二分插入省略号，避免按字符数截断导致宽度不准。
- 图标异步加载：`ui/icons.rs` 维护请求队列，工作线程调用 `ports::IconProvider`，完成后通过通道回到 UI 线程并缓存纹理；缓存上限 512，按“扩展名 / 目录 / .app 路径”分组。
- 只对可见行请求路径、元数据和图标；滚动时用 LRU 保留最近约 2000 行。
- 不使用 `egui_extras`、`egui_table` 等额外表格依赖，避免样式和行为分叉。

### 5.8 窗口模式（只作用于搜索页）

现状是单个 eframe 窗口：macOS 上被换成 `NSPanel`（`NonactivatingPanel` + popup 层级），Windows 上创建为置顶的 toolwindow，所有页面共用。因此“普通窗口”不需要每个页面重写，而是**进入搜索页时切换窗口属性，离开时恢复**。

| 模式 | 行为 | 实现 |
| --- | --- | --- |
| `Floating` | 置顶、失焦隐藏（与现有剪贴板面板一致） | 现有 `panel::show` 行为，`hide_on_blur=true` |
| `Pinned` | 置顶、不随失焦隐藏 | 同一窗口，`hide_on_blur=false` |
| `Normal` | 不置顶、可被其他应用覆盖、默认不随失焦隐藏 | 主模块调用 `panel::set_mode`：macOS 切 window level / collection behavior，Windows 切 `HWND_NOTOPMOST` |

关键边界：

- 只切换 `Page::Search` 的窗口属性；剪贴板面板、JSON 编辑器、OCR 仍使用原来的浮层模式，不存在“一改全改”。
- macOS 的窗口类仍是 `NSPanel`，`NonactivatingPanel` 和 `_setPreventsActivation` 在 attach 时设置。v1 的 `Normal` 只保证“不置顶、可被其他应用覆盖、不随失焦隐藏”，仍然是点击不激活本应用的搜索窗口；不会出现在 Dock 或 Cmd+Tab。
- Windows 的 `with_taskbar(false)` 是创建参数，运行中无法恢复任务栏按钮；v1 的 `Normal` 只保证不置顶，不会出现在任务栏。
- 如果要求 Dock / 任务栏按钮、标题栏、或者搜索窗口和剪贴板面板同时存在，应使用**独立视口**：在搜索模块内部用 `ctx.show_viewport_deferred` 创建第二个窗口，自己的 `ViewportBuilder` 设置 decorations / taskbar / window level，主模块只增加显示入口。这个方案仍然只影响搜索，工作量比页级模式多约 2–4 天。
- 建议顺序：v1 先实现页级 `Normal`（约 1–2 天），验证焦点和文件操作；确实需要 Dock/任务栏/双窗口时再升级独立视口，不需要推翻其他页面。
- `decorations=true` 时宿主发送 `egui::ViewportCommand::Decorations(true)`；默认 `false`，保持无边框圆角和现有玻璃背景。

### 5.9 中文输入法（IME）

- 组合期间不触发查询：搜索框收到 `egui::Event::Ime(ImeEvent::Preedit { .. })` 且文本非空时，设置 `composing=true`，暂停 debounce、停止发起新查询、保持旧结果；收到 `ImeEvent::Commit` 或空的 `Preedit` 后 `composing=false`，再按 debounce 搜索一次。
- `Esc` 在 `composing=true` 时交给输入法 / `TextEdit` 处理，不清空查询、不请求宿主隐藏；只有在非组合状态才执行“第一次清空、第二次隐藏”。
- `Enter` 同理：组合状态下交给输入法上屏，不提交搜索；上屏后的 `Commit` 触发搜索。
- egui 的 `TextEdit` 内部有 `is_ime_composition`，但不是公开 API；这里通过输入事件自行维护状态。egui 0.36 通常把 preedit 画在组合区、不写入绑定字符串，但不同平台/输入法的 `Preedit` / `Commit` 序列不完全一致；不要依赖“文本没变就不会查询”的偶然行为。
- 需要注意 `egui-winit` 文档列出的平台差异：有的平台会先发空 `Preedit` 再 `Commit`，有的会重复空 `Preedit`；状态机必须能处理这些序列。
- 验收：macOS 拼音、Windows 微软拼音各手工验证一次：组合期间结果不刷新、不闪烁；上屏后只发一次查询；组合中按 `Esc` 只取消候选，不隐藏窗口。

## 6. 搜索设置

### 6.1 文件与版本

- 路径：`<data_dir>/search/settings.json`。
- 使用 `serde` + `serde_json`，`#[serde(default)]` 兼容旧字段。
- 写入采用临时文件 + 原子 rename；解析失败时把原文件改名为 `settings.json.corrupt.<时间戳>`，使用默认值并在搜索页提示。

### 6.2 结构示例

```json
{
  "version": 1,
  "roots": [
    {
      "path": "/Users/me",
      "volume_id": "mac-volume-uuid",
      "enabled": true
    }
  ],
  "exclude_patterns": [
    "node_modules/",
    "target/",
    "DerivedData/",
    "*.log"
  ],
  "ignore_paths": [
    "/Users/me/Library/Caches"
  ],
  "allow_network_volumes": false,
  "scan_app_bundles": false,
  "watch": true,
  "lazy_start": true,
  "debounce_ms": 100,
  "default_case_sensitive": false,
  "show_all_on_empty": true,
  "max_results": 200000,
  "ui": {
    "window": {
      "size": [1100.0, 700.0],
      "mode": "floating",
      "decorations": false
    },
    "columns": {
      "name": 260.0,
      "path": 450.0,
      "size": 100.0,
      "modified": 155.0,
      "created": 155.0
    },
    "sort": { "column": "path", "descending": false }
  },
  "history": ["ext:rs search"],
  "saved_searches": []
}
```

- `roots`：每项是一个独立索引根；`volume_id` 用于拔插后匹配同一个卷，路径变化时仍能识别。
- `exclude_patterns`：相对每个根的规则；不含 `/` 时按 basename 匹配；末尾 `/` 只匹配目录。
- `ignore_paths`：绝对路径忽略项，优先级高于根内规则。
- `max_results`：交互查询返回的结果上限，默认 200,000；0 表示不限。超过上限时 UI 显示总数和“继续输入可收窄”。
- `lazy_start`：默认 `true`，首次打开搜索页时才加载快照 / 扫描。
- `ui.window.size`：搜索页自己的窗口大小，主模块通过 `window_policy()` 读取，不写主设置。
- `ui.window.mode`：`floating`（置顶 + 失焦隐藏）、`pinned`（置顶 + 不隐藏）、`normal`（不置顶 + 不隐藏）；`window_policy()` 据此计算 `hide_on_blur` 与窗口模式。
- `ui.window.decorations`：仅 `normal` 模式生效，是否请求标题栏；默认 `false`，保持无边框圆角。

### 6.3 设置变更策略

| 变更 | 行为 |
| --- | --- |
| 列宽、排序、窗口大小、隐藏策略 | 立即保存，立即生效 |
| 大小写默认、延迟、空查询行为、结果上限 | 立即保存，下一次查询生效 |
| 新增根 | 立即加入根列表，后台扫描新根，不重建其他根 |
| 移除根 | 取消该根扫描，移除其结果，立即生效 |
| 排除规则、忽略路径 | 需要“应用并重建”；重建全部根（v1 简化，v2 可做增量过滤） |
| 监听开关 | 立即停止/启动 watcher，不重建索引 |

## 7. 索引与查询

### 7.1 数据模型

```rust
type NodeId = u32;
type NameId = u32;
type RootId = u16;
/// (RootId, NodeId) 打包成 u64；跨根搜索结果、行分页和排序都用它。
type ResultId = u64;

struct Node {
    parent: NodeId,          // 根为 NONE
    name: NameId,            // 名字驻留表索引
    kind: NodeKind,          // File / Dir / Symlink / Package
    children: ThinVec<NodeId>, // 文件为空；目录使用
    size: u64,
    mtime: i64,
    ctime: i64,
}

struct NameEntry {
    text: Box<str>,          // 同名只保留一份
    nodes: ThinVec<NodeId>,  // 同名节点
}
```

- 名字驻留：所有同名文件共享一份 `NameEntry`；搜索时遍历“不同的名字”，再展开到节点。这比逐条记录扫描更省时间，也是 EverythingMac 在短查询上明显更快的原因。多根时名字表按根独立，跨根不共享，保证离线根可以被完整跳过。
- 路径不重复存储：通过 `parent` 链按需拼出；只对可见行拼路径。
- 目录子项：`ThinVec<NodeId>` 只对目录分配容量；文件为空。若内存仍需压缩，再改为 sibling 链或独立目录表。路径排序所需的 children 名字序在构建 rank 时统一排序，增量变更只把 rank 标记失效，不要求每次插入都维护全局有序。
- 元数据：大小/时间在扫描时尽量读取；读取失败的节点标记为“不可访问”，不阻塞其他项。排序/筛选需要时再补读。
- 4.29 亿节点是 `u32` 上限；远超本场景。超过时拒绝继续并提示。

### 7.2 内存目标

以 100 万条目为基准（实际以 Phase 0 测量为准）：

- 节点表 ≤ 60 MB；
- 名字表 + 名字文本 ≤ 25 MB；
- 结果集（100 万 `ResultId`，u64）≤ 8 MB；排序 keys 另计，100 万条约 12–16 MB；
- 快照 ≤ 15 MB（zstd 后）；
- 空闲 RSS 增量 ≤ 150 MB。

200 万条目按约 1.8 倍估算。索引常驻会增加 DevTools Hub 的内存占用，因此默认只索引主目录，并在搜索设置中提供“停止索引 / 释放索引”。

### 7.3 扫描

- 每个根独立扫描；使用工作队列 + N 个 worker（`std::thread` 或 `rayon`）。
- 从 `read_dir` 读取目录项；先做排除判断，再读取元数据，减少无用的 stat。
- 扫描只读取目录项和元数据，**不读取文件内容**；这是文件搜索，不是内容索引。
- 云占位文件：macOS 用 `st_flags & SF_DATALESS`（`0x40000000`）识别 iCloud / 云端未下载文件；Windows 用 `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`（`0x00400000`）和 cloud reparse tag 识别 OneDrive 占位文件。可以索引名称/元数据，但**不要对这些路径调用图标、缩略图、Quick Look 或打开 API**，它们可能触发 hydration；图标用扩展名通用图标。
- 如果默认根（主目录）包含 iCloud Desktop/Documents 或 OneDrive，首次扫描只做元数据枚举；设置页提示“检测到云占位文件，已跳过图标/内容读取”。若实测某个云目录连枚举都会触发下载，提供“跳过该目录”的根级开关。
- 不跟随符号链接；记录为 Symlink，不递归。
- macOS：默认跳过 `.app` 包内部；跳过 `/System/Volumes/Data`（根为 `/` 时），避免 APFS firmlink 重复。
- Windows：跳过 junction / reparse point 的递归，避免循环；默认跳过 `System Volume Information`、`$Recycle.Bin`。
- 扫描期间在旁路构建新的根索引，完成后原子替换该根；其他根保持可搜索。
- 进度事件包含：当前根、已扫描条目、目录数、错误数；UI 只按固定频率刷新。

### 7.4 排除规则（`node_modules`）

规则在扫描与增量事件两处生效：

1. **扫描时**：进入目录前检查该目录的路径/名字；命中则整个子树不遍历、不读元数据。
2. **监听时**：收到事件后检查事件路径及其所有祖先；祖先命中则丢弃事件，避免 `node_modules` 内部变更触发 walk。
3. **重建时**：规则变更后重建索引；旧索引里的排除项不会残留。

匹配语义：

- 规则不以 `/` 开头、不含 `..`；相对每个根。
- 不含 `/` 的规则按 basename 匹配，`node_modules` 命中任意层级的同名文件或目录；`node_modules/` 只命中目录。
- 含 `/` 的规则按相对路径匹配，支持 `*`、`?`、`**`。
- 大小写：默认不敏感（macOS / Windows 用户更直观），设置里提供“区分大小写”；底层匹配器遵循该选项。
- `ignore_paths` 使用绝对路径，优先于根内规则。
- v1 不做“排除后重新包含”的例外；需要例外时用户调整根或规则。

建议默认规则（可一键关闭）：

```
node_modules/
target/
DerivedData/
.git/
```

说明：排除 `node_modules` 会隐藏其中的包源码，适合大多数日常搜索；需要搜索依赖源码的用户可以在设置里删掉该条。`.git` 是否默认排除由评审决定，文档先按“默认排除”设计。

### 7.5 多根、外置硬盘与插拔

索引从一开始就是多根结构，不做“单根 + 以后改”的临时方案。

```rust
enum RootState {
    Offline,
    Scanning { scanned: u64 },
    Ready,
    Error(String),
}

struct Root {
    id: RootId,
    path: PathBuf,
    volume_id: Option<VolumeId>,
    state: RootState,
}
```

规则：

- 默认根：用户主目录（macOS `$HOME`，Windows `%USERPROFILE%`）；不默认索引整个 `/` 或所有盘符。
- 根必须在设置中显式添加；新增外置硬盘不会自动全盘索引。
- 启动时枚举挂载卷，根对应的卷不存在则标记 `Offline`，保留配置但不扫描、不出结果。
- **挂载**：macOS 监听 FSEvents 的 `Mount` 事件，并用 `NSWorkspace` 通知 + `getfsstat` 轮询兜底；Windows 监听 `WM_DEVICECHANGE` / `DBT_DEVICEARRIVAL`，并用 `GetLogicalDrives` 轮询兜底。匹配到已配置根后启动该根扫描。
- **卸载**：停止该根 watcher / 扫描，移除该根全部节点与名字引用，标记 `Offline`；结果代际递增，UI 立即丢弃旧行。
- **盘符变化（Windows）**：用卷 GUID 而不是盘符作为身份；盘符变化后按卷 GUID 匹配根，更新 `path` 后继续。
- **网络卷**：默认不索引；用户显式添加时，扫描设置超时与取消，避免 SMB 挂起拖住整个索引线程。
- **重新插入**：同一卷再次挂载时重新扫描该根（v1 全量；v2 可复用快照做增量）。
- 快照按根分文件保存（见 7.6），启动时只加载在线根；离线根的文件保留，不参与加载和搜索。

### 7.6 快照

快照不是单个 `index.bin`，而是“清单 + 每根一个自包含文件”，这样离线根可以跳过加载，移除根也不需要重写整份索引。

```
<data_dir>/search/index/
├── manifest.json          # 版本、根列表、根文件映射、更新时间
├── roots/
│   ├── <root_key>.bin     # 每个根自包含：名字表 + 节点 + 校验和
│   └── ...
└── *.tmp / *.corrupt      # 写入与损坏隔离
```

- `manifest.json`：格式版本、`RootId`、路径、卷标识、根文件名、根文件校验和、更新时间。写入用临时文件 + 原子 rename。
- `root_key`：优先使用卷 UUID / 卷 GUID；没有卷标识时用规范化路径的 SHA-256 前 16 位十六进制，避免路径分隔符进入文件名。
- 每个根文件自包含：magic、格式版本、根路径/卷标识、排除规则指纹、名字表、节点表、根内路径 rank、SHA-256。rank 带结构版本，与节点表不一致时加载后重建，不直接信任。
- 序列化：`postcard` + `zstd`；每个根单独压缩，不共享名字表。跨根同名文件会各存一份，换来根级独立加载/删除。
- 加载：只读取在线根对应的文件；离线根只保留 `manifest` 条目。某个根文件版本/指纹不匹配或校验失败，只重建该根，其他根不受影响。
- 保存：每个根独立写 `<root_key>.bin.tmp`，fsync 后 rename，再原子更新 `manifest.json`；失败保留旧文件并提示。
- 移除根：删除对应根文件并更新 `manifest`，不需要重写其他根；索引根数量少，这种删除是廉价操作。
- `manifest` 损坏或缺失：按设置中的根列表重建全部在线根；不尝试从残缺清单猜测。
- v1 每个根做全量快照，变更后 debounce 10 分钟保存一次，退出时保存；后续再考虑根内分页/WAL。
- 禁用“保存整个索引”时，只在内存中维护，退出丢弃；已有根文件保留还是删除由设置决定（默认保留，除非用户点“清除索引”）。

### 7.7 增量更新

- 每个根一个 watcher；事件先进入队列，按约 100–300ms 合并。
- 分类：
  - 创建 / 删除 / 重命名：重新 walk 该路径，按名字对比增删节点；
  - 仅属性变化（大小、时间、权限）：原位更新节点，节点 ID 不变；
  - `MustScanSubDirs` / 事件丢失 / 历史不可回放：重建该根；
  - `Mount` / `Unmount`：切换根状态；
  - 根路径本身变化：重建该根。
- 事件应用分步进行，每步处理一个目录或一批节点，期间允许搜索；步骤完成后再更新结果代际。
- 应用事件时逐级检查排除规则；被排除目录内部的事件直接丢弃。
- macOS 保存 FSEvents 的 `last_event_id`，启动时若历史仍可回放则追赶；否则重建。
- Windows `ReadDirectoryChangesW` 缓冲区溢出时重建该根。
- 重建 / 扫描期间，其他根仍可搜索；该根显示“更新中”。

### 7.8 查询语法

v1 支持 EverythingMac 常用子集，解析结果统一为 AST：

| 语法 | 说明 |
| --- | --- |
| `report` | 文件名子串，默认不区分大小写 |
| `"annual report"` | 短语，空格保留 |
| `report*.txt` | `*` 任意长度，`?` 单个字符 |
| `/report/` | 前缀 / 后缀 / 完整名字锚定 |
| `project/report` | 路径片段匹配 |
| `a b` | AND |
| `a \| b` | OR |
| `!a` | NOT |
| `(a \| b) !draft` | 括号分组 |
| `ext:pdf;txt` | 扩展名列表 |
| `type:picture` | 文件类型组 |
| `file:` / `folder:` | 仅文件 / 仅目录 |
| `size:>1MB` / `size:100KB-5MB` | 大小 |
| `dm:today` / `dc:pastweek` | 修改 / 创建日期 |
| `infolder:~/Documents` | 目录及后代 |
| `parent:~/Downloads` | 直接子项 |
| `regex:^report` | 正则（Aa 决定大小写） |

实现选择：

- **推荐**：把 EverythingMac 的 `everything-mac-syntax` 移植到 `core/query/syntax/`（纯 Rust、零依赖、MIT），保留版权声明；求值器自己实现，以便对接本模块的索引结构。该 crate 的 edition 与项目不同，移植时需要改成项目风格并去掉无关 doctest。
- **备选**：v1 自己实现上表子集（约 600–900 行），把解析器藏在 `core::query` 后面；以后需要完整语法时再替换。两种方式对外 API 不变。

求值约定：

- 普通词匹配文件名；含 `/` 的词、`path:` 匹配路径。
- 文件夹范围是独立字段，作为前缀过滤，不进入查询 AST。
- `Aa` 只影响词项、路径和正则的大小写。
- 空查询且 `show_all_on_empty=true` 时返回全部条目（路径序，受 `max_results` 约束），否则显示空态。
- 高亮只对词项 / 短语做字面量匹配；`regex:` 暂不高亮。

### 7.9 结果、排序与分页

- 搜索结果保存为 `Vec<ResultId>`，`ResultId` 由 `(RootId, NodeId)` 打包成 `u64`；只存 ID，不存路径或字符串。100 万结果约 8 MB，500 万约 40 MB，不是数百 MB。
- 节点 ID 顺序只作为内部存储，不作为用户可见排序；默认显示**路径序**。路径序由目录 children 的名字做一次 DFS，生成每个节点的 `u32` 根内 rank；多根时先按根路径排序，再比较根内 rank。全量扫描完成时顺带构建 rank，增量结构变化只把 rank 标记失效，下一次需要时在后台重建，期间显示旧结果和“排序中…”。
- 列排序在后台线程完成：
  - 名字：按名字文本比较；
  - 大小/修改/创建：预生成 `(key, ResultId)` 数组排序，避免逐次随机访问；
  - 路径：按上面生成的 rank 排序；不要复用节点 ID 顺序。
- 排序缓存只保留当前 `(generation, sort)` 的一份结果；新搜索 / 新排序替换旧缓存，避免快速输入时多份排序结果累积。
- 索引实时变化时把排序标记为 dirty，**空闲 500 ms–1 s 后重排一次**；期间继续显示旧排序，不随每个事件重排。
- 交互查询设置 `max_results`（默认 200,000，0 表示不限）。超过上限时按当前排序取前 N 项并返回总数，UI 显示“显示前 200,000 项，继续输入可收窄”；排序只作用于返回集合。空查询显示全部条目时同样受该上限约束。单字符查询（如 `e`）因此不会产生几十万条结果排序缓存。
- 大小/日期过滤需要元数据：对候选节点在后台补读，UI 显示“读取元数据…”，未读到的节点按未知处理，不阻塞其他结果。
- 100 万结果排序超过 100 ms 时，UI 显示“排序中…”，不阻塞输入；旧结果继续显示。
- 分页：UI 只请求可见范围 + overscan，服务端每次返回最多 128 行；行包含 ResultId、名称、路径、大小、时间、类型、图标键。
- 结果代际：每次搜索 / 索引替换递增；过期的行请求直接丢弃，避免快速输入时串行。

### 7.10 并发模型

```
UI 线程 ──Command──▶ Service 索引线程
   ▲                     │
   └────Event───────◀────┘（通过 waker 请求重绘）
```

- `Service` 独占索引；UI 不直接读索引。
- 扫描 worker 与搜索 worker 使用独立线程池；搜索结果只读取不可变快照（或加读锁），避免阻塞事件应用。
- 命令：`Search`、`LoadRows`、`SetSort`、`AddRoot`、`RemoveRoot`、`ApplySettings`、`Rebuild`、`Save`、`Shutdown`。
- 事件：`IndexStatus`、`SearchDone`、`Rows`、`RootsChanged`、`Error`、`Dirty`。
- 取消：`generation` + `CancellationToken`（原子标志）；旧查询在并行扫描的检查点退出。
- UI 每帧只 drain 事件，不做阻塞等待；搜索中按 16–50ms 请求重绘。

## 8. 平台层

### 8.1 `ports.rs`

```rust
pub trait FsSource: Send + Sync {
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryData>, FsError>;
    fn metadata(&self, path: &Path, follow: bool) -> Result<MetadataData, FsError>;
}

pub trait WatcherFactory: Send + Sync {
    fn start(&self, root: &Path, cb: Box<dyn Fn(Vec<RawEvent>) + Send + Sync>)
        -> Result<Box<dyn WatcherHandle>, WatchError>;
}

pub trait VolumeSource: Send + Sync {
    fn mounts(&self) -> Vec<MountInfo>;          // 路径 + 卷标识 + 是否网络卷
    fn volume_id(&self, path: &Path) -> Option<VolumeId>;
}

pub trait IconProvider: Send + Sync {
    /// `size` 是物理像素（UI 按缩放倍数换算），不是逻辑点。
    fn file_icon(&self, path: &Path, is_dir: bool, size: u16) -> Option<IconRgba>;
}

pub trait SystemActions: Send + Sync {
    fn open(&self, path: &Path);
    fn reveal(&self, path: &Path);
    fn open_terminal(&self, dir: &Path); // v2
}
```

- `IconProvider` 和 `SystemActions` 由 `ui` 通过 trait 使用，不直接碰平台代码。
- `Network` 标志用于默认跳过网络卷。

### 8.2 macOS

- 遍历：`std::fs::read_dir` + `symlink_metadata`；后续如扫描耗时成为瓶颈，再换 `getattrlistbulk`。
- 监听：`objc2-core-services` 的 FSEvents，使用 per-root stream，保存 `last_event_id`。
- 卷：`libc::getfsstat` 枚举；卷标识优先使用卷 UUID；FSEvents 的 `Mount` / `Unmount` 事件 + `NSWorkspace` 通知 + 轮询兜底。
- 图标：`NSWorkspace.iconForFile` → `NSImage` → `CGImage` → `NSBitmapImageRep` 转请求的物理像素 RGBA；后台线程加载，UI 只上传纹理。`SF_DATALESS` 文件不调用图标 API，直接返回扩展名通用图标。
- 打开/显示：`open` / `open -R`（与主模块现有做法一致，但实现放在本模块平台层）。
- 权限：完全磁盘访问；搜索页顶部提示并打开系统设置。不要为了权限自动请求管理员。
- 跳过 `.app` 内部；处理 `/System/Volumes/Data` firmlink 重复。

### 8.3 Windows

- 遍历：`std::fs::read_dir`（Windows 的 `DirEntry` 通常已带缓存元数据）；必要时后续换 `FindFirstFileW`。只读目录项/元数据，不打开文件；`FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` 的云占位文件照常索引名称，但不触发 shell 图标 / 缩略图 / 内容读取。
- 监听：`ReadDirectoryChangesW`（递归、overlapped 或独立线程 + 缓冲）；溢出/错误时重建根。
- 卷：`GetLogicalDrives`、`GetDriveTypeW`、卷 GUID（`GetVolumeNameForVolumeMountPointW`）；默认跳过 `DRIVE_REMOTE`。
- 插拔：优先 `RegisterDeviceNotificationW` + `WM_DEVICECHANGE`；实现成本过高时 v1 用 2–5 秒轮询盘符，文档中标注为降级方案。
- 图标：`SHGetFileInfoW` 取 `iIcon` 后，用 `SHGetImageList(SHIL_LARGE / SHIL_EXTRALARGE)` + `ImageList_GetIcon`，或 `IShellItemImageFactory::GetImage` 按请求的物理像素取图；不要只用 `SHGFI_SMALLICON`（固定 16px，2x 缩放下模糊）。按“扩展名 + 像素尺寸”缓存，`.exe/.lnk` 按路径缓存；云占位文件用通用扩展名图标。
- 打开/显示：`ShellExecuteW` / `explorer.exe /select,`。
- 长路径：内部统一 `PathBuf`；显示用 `to_string_lossy`，扫描遇到超长路径失败时计数并继续。

## 9. 错误处理、日志与安全

- 错误类型 `SearchError` 分为：配置错误、权限错误、扫描错误、快照损坏、监听错误、平台不可用。
- 单个根 / 单个目录的错误不致命：计数、记录、继续；状态栏显示错误数。
- 快照损坏绝不 panic：校验失败 → 改名 → 重建。
- 日志使用 `tracing`，target 为 `devtools_search`；路径只在 debug 级别记录，info 只记录计数/耗时/根状态。
- 不启动 HTTP / 网络服务；不读取文件内容；不上传数据。
- 只在用户显式添加的根内扫描；默认不碰其他卷和网络卷。
- 索引文件权限跟随用户数据目录，不额外放宽。

## 10. 测试与验收

### 10.1 单元 / 集成测试

- `exclusions.rs`：`node_modules` 任意层级、`node_modules/` 仅目录、`*.log`、`**/build/**`、大小写、绝对 ignore 路径、事件祖先命中。
- `query.rs`：词/短语/通配/布尔/过滤器、Aa、空查询、正则、文件夹范围；`max_results` 上限与总数字段。
- `sort.rs`：名字/大小/修改/创建排序；**路径排序在增量新增/删除节点后仍需与全量重建的路径序一致**；排序缓存只保留当前代。
- `snapshot.rs`：每根文件往返、只加载在线根、删除一个根不重写其他根、某个根校验失败只重建该根、manifest 损坏回退、原子写入失败。
- `volumes.rs`：根离线/在线、挂载匹配、卸载移除、盘符变化（模拟事件，不依赖真实设备）。
- 扫描：临时目录构造嵌套结构，断言节点数、名字去重、符号链接不递归、`.app` 剪枝；用合成属性验证 `SF_DATALESS` / `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` 被标记为占位文件且不请求图标。
- 监听：临时目录创建/删除/重命名；watcher 测试标记 `#[ignore]` 手动跑，避免 CI 不稳定。
- UI：`egui_kittest` 快照，使用合成数据（1 万行）验证表头、列宽、排序、空态、设置页；IME 组合状态机单独用事件序列测试（Preedit → Commit、空 Preedit → Commit）。
- 手工：macOS 拼音 / Windows 微软拼音；iCloud / OneDrive 占位目录扫描不触发下载；Windows 2x 缩放下图标清晰度。
- 性能：`#[ignore]` 测试生成 100 万合成节点，输出扫描/搜索/排序/快照耗时与内存估算；单字符查询验证 `max_results` 生效。

### 10.2 验收标准

1. `cargo check -p devtools-search` 无警告；`cargo test -p devtools-search` 通过。
2. 主模块改动只出现在 4.2 表列出的文件；`Settings` 只新增 `search_hotkey`。
3. `cargo test -p devtools-search layering` 通过（Windows / macOS 都跑，不依赖 rg）。
4. 在包含 `node_modules` 的临时目录中：
   - 扫描结果不包含 `node_modules` 内部文件；
   - 在 `node_modules` 内新建文件不触发索引更新；
   - 删除 `node_modules` 规则并重建后，内部文件出现。
5. 外置盘：
   - 添加根 → 插盘 → 自动扫描并出现结果；
   - 拔盘 → 根标记离线、结果移除、不崩溃；
   - 重新插盘 → 重新扫描；
   - macOS 与 Windows 各自至少手工验证一次（Windows 允许先用轮询方案）。
6. 100 万条目的搜索 p95 ≤ 30ms（本机实测，Phase 0 后校准）；UI 输入不卡顿；单字符查询不超过 `max_results`，并显示“继续输入可收窄”。
7. 关闭搜索页 / 退出应用后，搜索设置与快照落盘；下次打开恢复列宽、排序、窗口大小。
8. 增量新增/删除节点后，路径序与全量重建后的路径序一致；节点 ID 顺序不被当作路径序。
9. 中文 IME：组合期间不发起查询、不闪烁；上屏后只发一次查询；组合中按 `Esc` 只取消候选，不隐藏窗口。
10. 云占位文件：iCloud / OneDrive 目录扫描不触发文件下载；占位文件使用通用图标，不调用缩略图/打开 API。
11. 快照按根分文件：只加载在线根；删除一个根不重写其他根；某个根文件损坏时只重建该根。

## 11. 分阶段计划

> 工作量为单人估算，实际以 Phase 0 的测量结果校准。

| 阶段 | 内容 | 估算 |
| --- | --- | --- |
| M0 技术验证 | 临时 crate 扫描主目录；节点/名字结构；并行子串查询；100 万行 egui 表格；内存与耗时数据 | 2–3 天 |
| M1 模块骨架 | `devtools-search` 分层、设置读写、公开 API、主模块入口与快捷键、基础搜索页（无实时更新） | 4–5 天 |
| M2 索引闭环 | 扫描、排除规则、快照、分页、打开/显示、FDA 提示；主目录可用 | 5–7 天 |
| M3 实时与多根 | FSEvents / ReadDirectoryChangesW、增量更新、根状态、挂载/卸载、重建 | 5–7 天 |
| M4 体验完善 | 列排序、历史/保存搜索、设置页、图标缓存、空态、快照优化、页级 `pinned` / `normal` 窗口模式 | 4–6 天 |
| M5 Windows 对齐 | 扫描、监听、盘符/卷、图标、资源管理器、`windows-typecheck` | 4–6 天 |

内容搜索、拖出、Quick Look、独立视口（Dock/任务栏/双窗口）不在上述估算内；页级普通窗口模式已含在 M4 的 1–2 天内。

## 12. 风险与待定决策

### 12.1 风险

| 风险 | 影响 | 应对 |
| --- | --- | --- |
| 分层被破坏 | 后续无法独立测试/替换 | 3.4 跨平台 `layering.rs` + `pub(crate)` + 代码评审 |
| 全盘索引内存过高 | 常驻工具变重 | 默认主目录、懒加载、紧凑结构、可停止索引 |
| 首次扫描慢 | 用户以为没结果 | 进度、空态、先加载旧快照、旁路重建 |
| 路径排序与增量顺序漂移 | 用户看到错误排序 | 路径序 rank 按需重建，不复用节点 ID 顺序 |
| 单字符查询结果过大 | 排序/滚动卡顿 | `max_results` 默认 200k + 排序缓存只留当前代 |
| 中文 IME 组合触发查询 | 结果闪烁、Esc 冲突 | 5.9 组合期暂停查询，Esc 交给 IME |
| 云占位文件被 hydration | 首扫触发批量下载 | 只读元数据，跳过占位文件的图标/缩略图/内容 API |
| 快照单文件无法跳过离线根 | 外置盘策略落不了地 | 每根一个快照文件 + manifest |
| 表格/排序卡顿 | 输入不流畅 | 虚拟化、只取可见行、后台排序、结果代际 |
| 监听丢失/重命名复杂 | 结果过时 | 事件检查点、丢失即重建、状态提示 |
| 外置盘拔插残留 | 搜到不存在的文件 | 卸载移除子树、卷标识、轮询兜底 |
| 主窗口是浮层 | 与 EverythingMac 常驻窗口不同 | 5.8 的页级窗口模式只切搜索页；不重写其他页面。需要 Dock/任务栏/双窗口时再上独立视口 |
| Windows 未实机验证 | 平台差异 | `windows-typecheck` + 虚拟机/真机手工验收 |

### 12.2 需要评审拍板的点

1. 默认根：主目录（推荐）还是整个启动盘？
2. 默认排除：是否默认排除 `.git`；`node_modules` / `target` / `DerivedData` 是否默认开启？
3. 窗口行为：默认 `floating`，提供 `pinned` 与页级 `normal`（推荐，改动只作用于搜索页）；是否需要独立视口（Dock/任务栏/双窗口）？
4. 索引启动：首次打开搜索页时加载（推荐）还是应用启动即后台加载？
5. 查询语法：移植 `everything-mac-syntax` 还是 v1 只做子集？
6. 外置盘热插拔：v1 就做，还是先在 M3 做 macOS、Windows 放 M5？
7. 快捷键默认值：macOS `Cmd+Shift+Space`、Windows `Alt+Shift+F` 是否可接受？
8. 快捷面板入口：是否把选中的文本/文件作为初始查询传入（推荐文本作为查询、文件作为文件夹范围）。

## 13. 参考与取舍

### 13.1 参考仓库

| 项目 | Git 地址 | 本地参考目录 |
| --- | --- | --- |
| EverythingMac | <https://github.com/seedds/EverythingMac.git> | `temp_reference/EverythingMac-main` |
| MacEverything | <https://github.com/joshua-wu/MacEverything.git> | `temp_reference/MacEverything-main` |

```sh
# 仅用于对照阅读，不在本项目构建/运行
git clone https://github.com/seedds/EverythingMac.git
git clone https://github.com/joshua-wu/MacEverything.git
```

### 13.2 取舍

| 参考点 | 采用 | 说明 |
| --- | --- | --- |
| EverythingMac 五列表格、文件夹范围、状态栏 | 是 | 作为 UI 蓝本，用 egui 重写 |
| EverythingMac 唯一名字驻留 + 全量并行名字扫描 | 是 | 结构简单，短查询快 |
| EverythingMac 排除规则与多卷 Include 思路 | 是 | 改为本模块的多根 + glob 规则 |
| EverythingMac mmap 快照 / 排序索引 / 事件回放 | 参考 | v1 用全量快照 + 简化排序；后续按需引入 |
| EverythingMac SwiftUI/AppKit/C ABI | 否 | 与 egui 无关 |
| EverythingMac 整体 Rust 引擎 | 否 | nightly、macOS 专有扫描/FSEvents/UTType，Windows 缺失；只借鉴结构 |
| MacEverything trigram 名称/路径/内容索引 | 否（本期） | 功能强但复杂度高；内容搜索列为未来 |
| MacEverything WAL + 分页持久化 | 参考 | 以后索引规模变大再考虑 |
| MacEverything HTTP / MCP / daemon | 否 | 与工具集定位不符，且增加暴露面 |
| MacEverything 的排除缺失、固定单根 | 不采用 | 本方案从第一版支持排除与多根 |

许可证：EverythingMac 与 MacEverything 均为 MIT。若移植 `everything-mac-syntax`，保留原始 MIT 版权声明，并在模块顶部注明来源与修改；不复制 Swift / C++ 代码。

## 附录 A：审查问题处理

| 编号 | 结论 | 处理位置 |
| --- | --- | --- |
| P2 路径排序复用后端顺序 | **存在**。节点 ID 顺序会随并行扫描和增量事件漂移，不能当路径序 | 7.1 / 7.9：默认显示路径序，按 children 名字 DFS 生成根内 rank；结构变化使 rank 失效，按需重建；验收 8 |
| P3 单文件快照无法跳过离线根 | **存在**。单文件格式与“只加载在线根”矛盾，删除根也要重写整文件 | 7.5 / 7.6：改为 `manifest.json` + 每根一个自包含快照文件；按根加载/保存/删除/校验/重建 |
| P4 中文 IME 组合期触发查询 | **存在**。不能依赖 preedit 不写入字符串的偶然行为；Esc 语义也会冲突 | 5.9：组合期暂停查询，Esc/Enter 交给 IME；用 `Preedit` / `Commit` 事件维护状态；补 macOS/Windows 拼音验收 |
| P6 Windows 小图标模糊 | **存在（低）**。`SHGFI_SMALLICON` 固定 16px，2x 下不够 | 8.3：改用 `SHGetImageList` / `IShellItemImageFactory` 按物理像素取图；缓存键包含尺寸 |
| P7 云占位文件被 hydration | **部分存在**。仅枚举元数据通常不会下载；“扫描必然 materialize”不准确。真正危险的是图标、缩略图、Quick Look、内容读取 | 7.3 / 8.2 / 8.3：只读元数据；识别 `SF_DATALESS` 与 `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`；占位文件不调用图标 API；必要时根级跳过 |
| P8 分层脚本依赖 rg / 误伤注释 | **存在**。CI 的 Windows 机器不保证有 rg，注释里的路径也会误报 | 3.4：改为 `tests/layering.rs`，随 cargo test 跨平台运行；去注释、词边界匹配 |
| P9 单字符查询结果过大 | **部分存在**。结果只存 `ResultId`（u64），100 万约 8 MB，不是数百 MB；但排序 keys、单字符排序和缓存失效仍会卡 | 7.9：`max_results` 默认 200k、单份排序缓存、空闲重排；UI 提示继续输入收窄 |
