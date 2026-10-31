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

## 3. 未实现

| 项 | 说明 |
| --- | --- |
| 快照事件流 catch-up | 启动仍是"快照 → 全量校验"：先给出可查的旧数据，再后台重扫替换。事件流 id 已在检查点里预留，但没有做增量重放 |
| 监听能力受系统限制 | 网络盘 / 某些虚拟文件系统不支持 FSEvents 或 `ReadDirectoryChangesW`，此时提示"无法实时监听"，只能手动重建 |

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

搜索页快照输出在 `target/ui-snapshots/`：`search-page(.png/-dark)`、`search-settings(-dark)`、
`search-permissions(-dark)`、`search-empty`。
