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
- **UI**：Aa、历史下拉、搜索框（IME 组合期间不查询）、范围 chip、五列虚拟化表格
  （128 行一批 LoadRows）、列宽拖动、表头三态排序、右键菜单、键盘导航、
  状态栏（状态点/计数/耗时/浮动-固定）、设置面板（草稿 + 保存/还原）、首次引导页、toast。
- **宿主接入**：`search_enabled`/`search_hotkey`（默认关闭）、热键（关闭时注销）、
  托盘「文件搜索（未启用）」、快捷面板「文件搜索」、统一 `set_page`、
  搜索页不透明底/最小尺寸、`HostServices`（NSOpenPanel / SHBrowseForFolder、
  系统模态保护、打开/显示、拖动、重绘唤醒）、正常退出（托盘退出与更新重启不再 `process::exit`，
  `on_exit` 有界停止搜索线程）。

## 3. 未实现（按文档 §11 允许后置或属于 M3/M4/M5）

| 项 | 说明 |
| --- | --- |
| 文件系统实时监听 | `watch` 设置在界面里置灰并提示；变更后需手动「重建全部索引」。FSEvents / ReadDirectoryChangesW 接入点已在 `application/runtime.rs` 的扫描槽位预留 |
| 原生文件图标 | `HostServices::icon` 返回 `None`，界面画矢量图标 |
| 保存搜索 | `saved_searches` 字段保留，界面暂无入口 |
| 历史/列宽以外的 UI 细化 | 拖拽多选、Shift 连选、Cmd+A 未做（v1 为单选 + 方向键） |
| 压缩（compact） | `stale_ratio` 已计算，后台压缩任务未接 |
| 快照增量校验 | 现在启动即「快照 → 全量重扫」，未做事件流 catch-up |

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
