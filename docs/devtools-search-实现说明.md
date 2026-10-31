# devtools-search 实现说明（按第三版设计文档落地）

> 对应设计：`预研-devtools-search模块设计-第三版.md`（2026-10-30）。
> 本文只记录**这一轮实际写了什么、哪些没写、和文档哪里不一致**，方案本身以设计文档为准。

## 1. 代码结构

```
crates/
├── devtools-search/            外层 crate：ui / application / settings / platform
│   ├── src/lib.rs              HostServices trait、组合根导出
│   ├── src/application/        protocol（Command/Event）、config（投影）、runtime、service
│   ├── src/settings/           model（version 1 全字段）、store（原子写 + 损坏隔离）
│   ├── src/ui/                 mod（模块状态机）、header/results/status/settings_panel/
│   │                           permissions/history/cache/theme
│   ├── src/platform/           fs（StdFs）、volumes（卷身份/挂载）、actions（打开/显示）
│   └── core/                   devtools-search-core（纯核心，无 UI/平台依赖）
│       └── src/                model/index/query(ast,parse,eval,sort,types)/scan/
│                               snapshot/volumes/ports/text
└── devtools-hub/               宿主：新增 Page::Search、HostBridge、平台层能力
```

依赖方向：`ui → application → core / platform / settings`；`core` 不依赖 egui、宿主或外层 crate
（`cargo check -p devtools-search-core` 可独立通过）。

## 2. 已实现（M1 骨架 + M2 索引闭环的主干）

- **索引**：SoA 列存储（parent/name/kind/status/size/mtime/btime）、共享名字 arena
  （raw/NFC/lower 仅在不相同时另存一份）、子节点平面表、DFS rank、欧拉区间；
  `revision` 每批变更递增，扫描完成整体替换并 `epoch+1`；NodeId 同代际内不复用。
- **查询**：`report`、`"annual report"`、通配符、`(a | b) !draft`、`project/report`、
  `ext:`、`type:`、`file:`/`folder:`、`size:>1MB`、`dm:`/`dc:`（today/yesterday/thisweek/
  thismonth/thisyear/pastweek/pastmonth/pastyear 与 ISO 日期、`a..b` 区间）、`infolder:`、
  `parent:`、`regex:`；`path:`/`content:` 等明确报错而不是悄悄忽略；未知 `type:` 组报错。
- **排序**：默认路径序用 rank（绝不用 NodeId 顺序），显式列排序跨根全局排序，未知元数据统一置后。
- **扫描**：扫描时预取元数据、交叉卷不递归、符号链接不跟随、`.app` 可整体跳过、
  排除规则命中目录不 walk 内部（节点留 `Excluded` 状态以便解除规则恢复）。
  排除规则语义（2026-10-30 确认）：**从根目录开始写完整路径才是根相对**——单段规则
  （`node_modules/`、`Library/`、`*.log`）一律任意层级匹配，结尾 `/` 只表示目录；
  多段完整路径（`AppData/Local/Temp/`）或前导 `/`（`/Library/`）才按根相对。
- **设置**：version 1 全字段、未知字段容忍、原子写（tmp → fsync → rename）、
  损坏隔离为 `.corrupt.<ts>`、更高版本只读。
- **服务**：唯一设置所有者；会话/查询代际过滤过期结果；索引指纹（epoch/revision/节点数）
  变化自动重跑查询；`lazy_start`、`continue_when_hidden`、暂停/恢复、
  添加/移除根、重建、清缓存、快照落盘与清理。
- **实时监听**：每个根用 `notify`（macOS FSEvents / Windows `ReadDirectoryChangesW`）递归监听，
  事件按 350ms 合并成一批后增量更新索引（新增 / 更新 / 整棵子树删除），提交一次 revision；
  墓碑超过 30% 时顺带压缩（换 epoch）；事件溢出或监听不可用时标记 `Dirty` 转全量重扫，
  监听启动失败只提示一次不反复重试。根路径会被规范化（`/var` → `/private/var`），
  保证事件路径能和索引根对上。
- **原生文件图标**：`HostServices::icon` 由宿主实现（macOS `NSWorkspace`、Windows `SHGetFileInfo`），
  模块按扩展名 / 目录缓存 32px 纹理、按 18px 绘制，取不到时回退矢量图标。
- **保存搜索**：顶栏星标入口，保存当前查询 / 范围（去重、上限 20），一键应用或删除。
- **多选**：Shift 点击 / Shift 方向键连选、⌘A 全选、Esc 先取消选区；
  选中多行时右键与 ⇧⌘C 复制全部路径，状态栏显示"已选 N 项"。
- **UI**：Aa、历史下拉、搜索框（IME 组合期间不查询）、范围 chip、五列虚拟化表格
  （128 行一批 LoadRows）、列宽拖动、表头三态排序、右键菜单、键盘导航、
  状态栏（状态点/计数/耗时/浮动-固定-普通）、设置面板（草稿 + 保存/还原）、首次引导页、toast。
- **宿主接入**：`search_enabled`/`search_hotkey`（默认关闭）、热键（关闭时注销）、
  托盘「文件搜索（未启用）」、快捷面板「文件搜索」、独立搜索窗口（见 §2.1）、
  `HostServices`（NSOpenPanel / SHBrowseForFolder、
  系统模态保护、打开/显示、拖动、重绘唤醒）、正常退出（托盘退出与更新重启不再 `process::exit`，
  `on_exit` 有界停止搜索线程）。

### 2.1 搜索页的独立普通窗口（2026-10-31）

搜索页不再和剪贴板面板共用一个无边框置顶窗口，改由宿主用 `show_viewport_deferred`
创建独立视口（`crates/devtools-hub/src/ui/search_window.rs`）。

- **三种模式**：`Normal`（默认，带系统标题栏、可最小化、不置顶、进入任务栏 / Dock）、
  `Pinned`（无边框、始终置顶、失焦保留）、`Floating`（无边框、始终置顶、失焦收起）。
  状态栏胶囊按 浮动 → 固定 → 普通 循环，设置面板可直接选择。
- **与面板解耦**：搜索窗口的开合不再经过 `Page::Search`，也不占用共享窗口的页面状态；
  搜索打开时先收起临时浮层，之后仍可单独打开剪贴板面板（两个窗口并存）。
  `Page::Search` 只留给离屏渲染测试直接绘制搜索界面。
- **创建时机**：面板隐藏时 eframe 只调用 `logic`，打开搜索先用 `panel::wake()`
  让面板以隐藏状态跑一帧界面，登记延迟视口后保持唤醒，直到搜索窗口关闭。
  关闭后停止调用 `show_viewport_deferred`，egui / eframe 自动销毁原生窗口。
- **为什么用延迟视口**：立即视口（`show_viewport_immediate`）要求父窗口在界面回调里同步
  创建原生窗口，而 eframe 在定时重绘隐藏窗口的路径上没有事件循环上下文，创建会失败并 panic
  （`egui backend is implemented incorrectly - the user callback was never called`）。
  延迟视口的窗口由 eframe 在事件循环里创建，搜索窗口也能脱离父窗口独立重绘。
  模块因此放在 `Arc<Mutex<SearchModule>>` 里，宿主与视口回调共享；
  `Service::handle` 每条命令都会唤醒宿主，宿主 `poll` 后把重绘转发给搜索视口。
- **macOS**：普通模式临时把应用的激活策略从 Accessory 切到 Regular（出现 Dock 图标，
  关闭后恢复）。没有 `.app` 包时（`cargo run` / target 下的可执行文件）Dock 的通用图标
  来自可执行文件本身：启动时把 `assets/AppIcon.icns` 同时设成 `NSApplication.applicationIconImage`
  和可执行文件的文件图标，Dock 立即显示正确图标；带 `.app` 包时跳过文件图标。
  文件夹选择器关闭后不再把焦点还给之前的应用。
- **退出**：macOS 上 `run_native` 返回后直接 `_exit(0)`。输入法 / objc 的线程局部析构
  在进程退出时会 panic（`fatal runtime error: thread local panicked on drop`），此时
  状态保存、搜索线程停止、更新重启的子进程都已完成，不需要再执行这些析构。
- **空态**：无论有没有结果，表头和底部状态栏都保留（空态提示在结果区居中）；
  搜索进行中空态显示“搜索中…”，不在“空白表格”和空态之间来回切换，
  避免搜索过程中中间区域和底部状态栏闪动；行数据未加载时也不画空卡片。
- **文件夹选择**：同一原生选择器按用途给不同提示——添加索引根是“选择要建立索引的文件夹”，
  设置搜索范围是“选择要搜索的文件夹”。
- **Windows**：任务栏按钮由视口创建参数决定；模式切换导致任务栏属性变化时由 eframe 重建窗口。
- **设置迁移**：`SETTINGS_VERSION` 升到 2；version 1（只有浮动 / 固定）的配置在加载时
  统一升级为普通窗口，其余字段保持原值。
- **尺寸与位置**：打开时出现在鼠标所在屏幕中央，会话内模式切换不会把窗口挪回中央；
  用户缩放后的内容区尺寸在关闭时写回设置。

### 2.2 结果区的视图切换与类型筛选（2026-10-31）

参照 Everything：结果区上方加了一条 34px 的工具条（`ui/toolbar.rs`），左边是「类型」下拉，
右边是「视图」下拉。两者都是"按钮 + 浮层列表"，浮层用 `egui::Area`（Foreground 层、
与历史 / 保存搜索下拉同一套外观），点空白处、Esc、选中一项后关闭。

- **类型筛选是查询条件，不是结果过滤**：`Command::SetKind` 写进 `settings.ui.kind`，
  `Service::search` 把它放进 `QuerySpec.kind`，`run_query` 再用 `FileKind::to_ast()`
  与查询 AST 做 AND。因此它和关键词、布尔语法、`ext:` 等过滤器可以自由组合；
  空关键词 + 具体类型也生效（`show_all_on_empty=false` 时仍然查询），
  改类型会重跑当前查询，还没搜索过时按"空关键词 + 该类型"查一次（顺带建索引）。
- **类型表的扩展**（`core/query/types.rs`）：新增 `executable` 组
  （exe/msi/msix/com/app/dmg/pkg/deb/rpm/AppImage/bat/cmd/ps1/sh/run），
  video / audio / archive / doc 补了常见扩展名；`picture` 的集合按设计文档保持固定。
- **视图只影响绘制，不重跑查询**：三种图标视图按列铺卡片、按行虚拟化
  （`show_rows` 一次给一整行的可见范围，再换算成结果下标去按批 `LoadRows`）。
  列数存在 `ViewState::grid_cols`，键盘上下移动一整行要用它。
- **两种视图共用一套选中状态**：`cursor` / `anchor` 都是结果下标，切换视图不丢选中；
  各自的 `ScrollArea` 用不同的 `id_salt`，来回切换保留各自的滚动位置。
- **图标视图的文字对齐**：`LayoutJob` 设了 `halign = Center` 之后，绘制原点要传卡片
  **中线**而不是左边——否则整段文字左移半个行宽，文件名只剩后半截（第一次快照就是这个样子）。
- **顺带修正**：索引根节点自身不再作为结果（按 `RootIndex::root_node()` 过滤）。
  此前搜根目录名、或按「文件夹」筛选时，结果里会多出一条"就是搜索范围本身"的项。
- **持久化**：`settings.ui.view` / `ui.kind`，靠结构体上的 `#[serde(default)]` 兼容旧设置。
- **快捷键**：⌘⇧1…4（Windows 为 Ctrl+Shift+1…4）切视图，与设计稿一致；
  提示文字走 `devtools_ui::keys::shortcut`，两个平台的写法不同。

### 2.3 预览缩略图、系统右键菜单与焦点修正（2026-10-31）

**缩略图**（图标视图里的"预览"，与资源管理器一致）

- 接口：`HostServices::thumbnail` → 宿主 `platform::thumbnail` → Windows
  `IShellItemImageFactory::GetImage`，`SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK`。
  走的是系统同一套缩略图提供者：图片、视频首帧、PDF / Office 文档、装了预览器的第三方格式
  都会出预览；`THUMBNAILONLY` 保证没有预览时**直接失败**，而不是拿文件类型图标冒充预览。
- 回退链：系统预览（`THUMBNAILONLY`）→ **按尺寸渲染的系统图标**（`ICONONLY`）→
  `image` 自己解码（只覆盖 png/jpeg/gif/bmp/webp/tiff）→ 矢量图标。
  第二级很关键：图标视图以前回退到 `SHGetFileInfoW` 的 32×32 小图标，放大到 96 全是糊的；
  `ICONONLY` 由 shell 直接按目标尺寸出图（大图标会给高分辨率资源），和资源管理器的图标一样清楚。
  macOS 对应 `NSWorkspace::iconForFile`（支持任意尺寸）。
- **解码在后台线程**（`ui/thumb_cache.rs`）：两个 STA 线程，每个线程一个队列，投递时轮流分配；
  解完在后台线程直接 `Context::load_texture` 并回传句柄，UI 每帧 `try_recv` 收。
  缓存键是 (路径, 目标尺寸)，上限 200 张，取不到也记 `None`（否则每帧重试）。
  请求尺寸按 `pixels_per_point` 放大到 2× 以内，Retina 下不发虚。
- 目录也走同一条路：`platform::thumbnail` 先看 `metadata`，目录直接跳过预览、要 `ICONONLY`
  的文件夹图标（省一次 shell 调用）；否则图标视图里的文件夹会退回矢量图，和资源管理器不一致。
- 绘制：`paint_thumbnail` 等比放进卡片（不裁剪、不变形）。**解好之前画矢量图标占位**，
  不再用 32×32 的系统小图标——否则每张卡片都要先糊一下再变清晰。

**结果区排版**

- 图标网格的列数按 `n * cell + (n-1) * gap ≤ 宽度` 反推（旧公式少算最后一个间距，会白白少放一列）；
  取整后剩下的宽度全部摊进两两之间的间距，整行铺满、右边不留一大条空白。
- 状态栏下面留 `BOTTOM_PAD`（10px），并且在结果区高度里先扣掉它——否则状态栏会贴着窗口底边，
  Windows 上文字和胶囊还会被裁掉一截。
- 有选中项时状态栏左侧显示它的**完整路径**（中间省略，悬停看全文），没有选中才显示索引状态。

**右键菜单**

- Windows 用系统菜单：`IShellItem` → `BindToHandler(BHID_SFUIObject)` → `IContextMenu`
  → `QueryContextMenu` → `TrackPopupMenuEx(TPM_RETURNCMD)` → `InvokeCommand`。
  「打开方式」「发送到」以及 Git、7-Zip、压缩软件、手机互传等第三方扩展全都在。
- 菜单顶部再插三条固定项（命令 id 从 `0x8000` 起，避开 shell 的 `1..0x7FFF`）：
  **打开 / 打开路径 / 复制完整路径和文件名**，选中后分别走 `Command::Open`、`Command::Reveal`
  和 `ctx.copy_text`——和键盘操作同一条路，不是另写一套。
- 它是**模态**的：`TrackPopupMenuEx` 自己跑消息循环，选完才返回。调用点放在结果区绘制完成之后
  （`SearchModule::ui` 末尾），这一帧的提交会推迟到菜单关闭，屏幕上是上一帧画面。
- macOS 继续用 egui 画的菜单，由 `HostServices::has_system_context_menu` 决定走哪条。
- 系统菜单是原生窗口，**离屏快照抓不到**；验证靠
  `shell_builds_a_context_menu`（能 Query 出菜单项）和手工运行。

**焦点修正**

- 搜索框有焦点时 ⌘A 归输入框（全选文字），结果区不再抢；图标视图的 ←→ 同理。
  `ViewState::search_focused` 由顶栏每帧写入，结果区在键盘处理里读取。

**依赖代价**：`windows` crate 增加 `Win32_Foundation`、`Win32_Graphics_Gdi`、
`Win32_System_Com`、`Win32_UI_Shell` 四个特性（缩略图与右键菜单共用）。macOS 侧没有新增依赖，
QuickLook 缩略图（`QLThumbnailGenerator`）还没接，macOS 目前靠 `image` 解码兜底。

### 2.4 结果区的选中、排序与状态栏（2026-10-31）

**选中模型换成集合**

- `ViewState::selected: BTreeSet<usize>` 取代原来的 `(anchor, cursor)` 区间模型：
  区间只能表达"连续一段"，做不了 Ctrl 点选、框选这种不连续的选择。
- 交互约定（和资源管理器一致）：
  | 操作 | 结果 |
  | --- | --- |
  | 单击 | 只选这一项 |
  | Ctrl / ⌘ + 单击 | 加入 / 移出选择 |
  | Shift + 单击 | 从锚点连选到这一项 |
  | 点空白 | 取消选择 |
  | 空白处按住左键拖 | 框选（框到的都选中），按住 Ctrl 是追加框选 |
  | ⌘A / Esc | 全选 / 先收成只选当前项 |
- 框选：在卡片**之前**注册一个覆盖结果区的空白层（交互是后注册的在上层，卡片才接得住自己的点击），
  拖动时用**上一帧**记下的 `hit_cells` 做命中测试——差一帧在拖动过程中看不出来，
  但省掉了"内容先画、交互后抢"的死结。
- 结果集增量刷新时用 `clamp_selection` 只丢掉越界的下标，用户选中的项尽量留着；
  但**换查询、换筛选、换排序、换视图**都会清空选中（结果集含义变了，下标不再代表同一批东西）。

**索引变化后的重跑要节流**

- 原来"索引指纹一变就整表重跑"：扫描和监听是连续变化（revision 一直在涨），
  框选、滚动这类持续重绘的场景里每帧都会触发一次查询。大索引一次查询要一秒，
  结果是查询风暴 + CPU 占满 + 界面卡成幻灯片（状态栏的耗时数字一直在跳）。
- 现在三道闸：正在扫描 / 校验的根直接跳过（等扫完再说）；第一次发现变化后等
  `RERUN_SETTLE`（400ms）让索引安静下来；两次重跑之间至少隔
  `max(RERUN_MIN_GAP(600ms), 上次查询耗时 × 1.5)`——间隔跟着查询耗时走，
  否则新查询会把上一次取消掉，结果永远出不来。
- 框选拖动期间缩略图只读缓存、不发起新请求（解码线程抢 CPU 会让拖动发飘）。

**排序下拉**

- 放在视图切换的左边，列出 名称 / 路径 / 大小 / 修改时间 / 创建时间，当前列打点并显示 ↑↓。
- 值直接读 `settings.ui.sort`，所以**表头点击后下拉的显示自动跟着变**；
  反过来下拉选完发的也是同一条 `Command::SetSort`，两条路完全一致。
- 路径列是"默认顺序"，没有升降之分（`UiSort::dir()` 对 Path 恒为 `None`）。

**状态栏**

- 有选中项时左侧显示 `图标 + 完整路径（中间省略）+ · 文件大小`，没有选中才回到索引状态。

**网格排版**

- 左右各留 `GRID_SIDE_PAD`（10px），第一列和最后一列不贴边；
  列数按扣掉两侧留白后的宽度算，余量继续摊进列间距。

### 2.5 性能：CPU 与内存的三处浪费（2026-10-31）

用 20 万节点的合成索引做了基准（`cargo test -p devtools-search-core --release -- --ignored`），
数字很说明问题——**查询算法本身不慢，慢的是重复劳动**：

| 阶段 | release | debug（改之前） | 说明 |
| --- | --- | --- | --- |
| eval（全量匹配 20 万） | 1.2ms | 7.4ms | 遍历本身很快 |
| 名称排序（全量 20 万 → 1000） | 41ms | 261ms | 全量排序是大头 |
| 名称排序（top-k 改后） | 22ms | 26ms | 先线性划分再排前 1000 |

三处修复：

1. **dev 构建给自己的代码开 `opt-level = 1`**（根 `Cargo.toml`）。依赖一直是 2，
   只有本项目代码是 0，所以 debug 下搜索的一次排序要 260ms、界面跟着顿。开 1 之后
   同一基准从 261ms 降到 26ms（约 10 倍），行号信息还在。
2. **排序改 top-k**：只要前 `limit` 个时先 `select_nth_unstable` 线性划分，再对前 limit 个排序。
   比较函数末尾用 rank 兜底、保证严格全序，划分不改变结果顺序。
3. **图标按扩展名复用**（`ThumbnailCache`）：`.rs` 的图标全都一样，按 (扩展名, 尺寸) 缓存后，
   滚动几百个源码文件只有第一个要问 shell。之前每个文件都要跑
   `THUMBNAILONLY` + `ICONONLY` 两次 COM 调用再加解码，滚动两下就能把两个核跑满，
   缓存也涨到 200 张预览（约 30MB）。
4. **监听同步加签名缓存**：`sync_watchers` 原来每帧都重新构造配置、编译一遍 globset
   （几十条规则），现在只有配置或根状态变化时才做。

另外加了开关 `auto_refresh`（设置 → 索引行为 →「索引变化时自动刷新结果」，默认开）：
关掉后索引怎么变都不重跑查询，要重新搜一次才看到新文件——大索引下能省掉反复重查的开销。

## 3. 未实现

| 项 | 说明 |
| --- | --- |
| 快照事件流 catch-up | 启动仍是"快照 → 全量校验"：先给出可查的旧数据，再后台重扫替换 |
| 监听能力受系统限制 | 网络盘 / 某些虚拟文件系统不支持 FSEvents 或 `ReadDirectoryChangesW`，此时提示"无法实时监听"，只能手动重建 |
| 关窗期间的增量补齐 | 关窗已经停监听，但下次打开是全量重扫；能不能改成增量见 §3.1 |

### 3.1 关窗期间的增量补齐（评估，2026-10-31）

**现状**

| 项 | 行为 |
| --- | --- |
| 关闭搜索窗口 | `VisibilityChanged { visible: false }` → `set_visible(false)`。`continue_when_hidden = false`（默认）时：停监听 → 取消扫描 → 根标记 `Dirty` |
| 重新打开 | `set_visible(true)` → 对 `Dirty` / `Paused` 的根 `spawn_scan` → **全量重扫**。快照先顶上（结果不会是空的），但校验是全量的 |

所以「关窗后不再后台监听、不更新索引」这条**已经是现状**，缺的只是「下次打开别全量重扫」。

**能不能拿到关窗期间的增量事件**

| 平台 | 能力 | 结论 |
| --- | --- | --- |
| macOS | FSEvents `FSEventStreamCreate` 的 `sinceWhen` 就是为这个设计的：存下最后一次的事件 id，下次建流时传进去，系统把期间的变更回放出来 | 可行，改动不大 |
| Windows | `ReadDirectoryChangesW` 是实时流，句柄一关就没有历史；要回溯只能用 **NTFS USN Journal**（`FSCTL_QUERY_USN_JOURNAL` 拿 journal id，`FSCTL_READ_USN_JOURNAL` 从上次的 USN 往后读） | 可行，但只覆盖 NTFS；journal 被删 / 重置、卷不支持时要回退全扫 |
| 通用兜底 | 先载入快照（秒开）+ 按 mtime/size「快速校验」：目录没变就整棵子树跳过 | 不是真增量，但比全扫快一个数量级 |

**建议顺序**

1. 先做通用兜底（收益 / 成本比最高，体感是"打开即出结果、几秒后静默更新"）；
2. macOS 上接 FSEvents `sinceWhen`（事件 id 存进快照检查点）；
3. Windows 上接 USN Journal，并做好"journal 不可用 → 回退全量"的兜底。

`snapshot.rs` 的 `EventCheckpoint` 已经预留了事件 id 的位置，这三步都不用改快照格式。

## 4. 与设计文档的偏差（需要在评审时确认）

1. **排除规则的层级语义（已确认，不再是偏差）**：文档里「`node_modules` 任意层级」与
   「`Library/` 只命中顶层」在语法上形状相同，经确认采用：
   **要按根相对匹配，必须写成从根目录开始的完整路径**（多段 `AppData/Local/Temp/`，
   或前导 `/` 如 `/Library/`）；**其余情况（含单段 + 结尾 `/`）一律按任意层级匹配**，
   结尾 `/` 只表示"只匹配目录"。默认排除表因此回到文档字面写法
   （macOS：`node_modules/`、`target/`、`DerivedData/`、`.git/`、`Library/`），
   它们会剪掉任意层级的同名目录；只想排除顶层时写成 `/Library/`。
2. **Windows 卷身份**：用盘符根（`win-drive:C:\`）而非卷 GUID——`windows-sys 0.61` 没有导出
   `GetVolumeNameForVolumeMountPointW`。macOS 用 `st_dev`（跨重新挂载可能变化），
   稳定 UUID 需要 DiskArbitration，属于 M0 待验证项。
3. **创建时间**：macOS 的 `st_birthtime` 没有 std 接口，`btime` 为 Unknown（列显示「—」、排序置后）；
   Windows 用 `Metadata::created()`。
4. **窗口空格行为**：`set_search_space_behavior` 在两端均为空实现——宿主没有全局绑定空格，
   面板已是 101 层级；入口保留给后续原生实现。
5. **名称词大小写**：不区分大小写用 Unicode lowercase（非完整 case folding，文档已注明）。

## 5. 验证方式

```bash
cargo check --workspace                                   # 无警告
cargo test --workspace                                    # 231 个测试
cargo test -p devtools-search --test service_integration   # 扫描→索引→查询→取行→快照
cargo test -p devtools-hub snapshot -- --ignored           # 含 search-page(-dark) 快照
cargo check -p devtools-hub --features windows-typecheck   # Windows 实现类型检查
```

搜索页快照输出在 `target/ui-snapshots/`：`search-page`、`search-page-grid`（图标视图）、
`search-page-grid-dark`、`search-page-grid-kind`（类型下拉展开）、
`search-page-grid-view`（视图菜单展开）、`search-settings(-dark)`、
`search-permissions(-dark)`、`search-empty`。
