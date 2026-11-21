# 《devtools-search 模块设计（第二版）》评审意见

> 评审对象：`docs/预研-devtools-search模块设计-第二版.md`
> 评审基线：`679ab29`（本次对照读了 `settings.rs`、`hotkey.rs`、`controller.rs`、`tray.rs`、`ui/mod.rs`、`platform/{macos,windows}/panel.rs` 与 `floating.rs`、`tools/mod.rs`，以及 eframe 0.36、muda 0.21 的接口）
> 评审日期：2026-10-30
> 结论：**可以进入 M0**。上一轮的 32 项基本都有落点，而且纠正了评审本身的两处错误。但有两项"宿主能力"必须在公开 API 里定下来，否则 M1 阶段"模块自包含"会被迫破例；另有几处内部语义需要先对齐，属于"现在改文档、以后不用返工"的范畴。
> 说明：本次只做静态评审，没有构建或运行。涉及窗口层级、模态对话框、TCC 提示等系统行为的结论，需要在 M0 实机确认。

## 一、总体判断

第二版比第一版收敛得多：

- 默认关闭、Service 单写配置、`IndexConfig` 投影、嵌套核心 crate，把"模块自包含"从口号变成了可检查的边界。
- Floating / Pinned 两种窗口模式、Normal 推迟到 v2，砍掉了页级平台切换的大半工作量。
- 墓碑不复用、`index_epoch`、按根 `(root_id, epoch, revision)`、事件检查点与快照同文件、Windows 冷启动校验，这些都是第一版缺失、且做错后很难补的部分。
- 验收和性能口径写成了可反驳的条目（含"未达标属于 M0 结论"），比"目标 30ms"这种写法有用。
- 附录 A 逐项说明处理位置，P2-25 还纠正了评审的前提：`resolve_data_dir()` 确实用的是 `data_local_dir()`（`settings.rs:47`），Windows 本来就在 LOCALAPPDATA。u32 上限、"结果集数百 MB"这两处评审错误也被改对了。

剩下的问题集中在两类：

1. **模块边界与宿主窗口的交界**：文件夹选择、权限引导、退出都在 v2 里被划进模块，但它们都需要操作宿主的窗口和应用激活状态，而现有公开 API 没有这个通道。
2. **增量更新下的呈现语义**：行批次和 revision 的绑定过严、墓碑一词被用在三种不同含义上、高亮和 NFC 映射没有算成本。

## 二、上一轮意见的复核

| 上一轮 | 复核结论 |
| --- | --- |
| P0-1 TCC | 引导、拒绝目录计数、签名前置条件都写到了；但弹出系统对话框的窗口层级/时序问题没处理，见本意见 §3.2 |
| P0-2 拔盘 | 设备线程、`DBT_DEVICEQUERYREMOVE`、取消扫描/关闭句柄、验收都补上了；M0 实机仍是前置 |
| P0-3 Windows 校验 | 策略具体（先可查、标 Verifying、全量重扫后替换），USN 归 v2 |
| P0-4 root_key | 改为"卷身份 + 相对路径哈希"，根与 ignore 用同一 `RootLocation`，正确 |
| P0-5 嵌套根 | 明确拒绝并提示，不自动维护双向排除，可接受 |
| P0-6 自目录 | 运行时派生的不可删除忽略项，删配置字段也解不掉，比写在默认规则里可靠 |
| P1-7 策略通知 | 有 `policy_changed`；但 `poll()`/`ui()` 两个返回值的合并语义、对话框期间的时序没定，见本意见 §3.2、§4.1 |
| P1-8 切页收口 | 意图正确，但没有点出 `controller.rs` 仍会直接 `panel::hide`，见本意见 §4.6 |
| P1-9 FolderPicker | 方向对，但它是 P0 级宿主能力缺口，见本意见 §3.2 |
| P1-10 Normal | 推迟到 v2 是对的；代价是搜索页继承现有浮层的层级/集合行为，这个"隐含接受项"要写明，见本意见 §4.4 |
| P1-11 节点生命周期 | 墓碑不复用、代际校验都写了；行批次校验过严和"三种墓碑"混淆见本意见 §4.1、§4.2 |
| P1-12 并发 | 每根 RwLock、专用池、短批写锁、扫描优先级都定了，方案可落地 |
| P1-13 元数据 | 扫描预取、Windows 用枚举自带元数据、macOS M0 对比，正确 |
| P1-14 检查点 | 与节点数据同一份快照、代际文件 + manifest，正确 |
| P1-15 NFC | 必选，大小写规则写清了；高亮映射的成本见本意见 §4.3 |
| P2-16 ~ P2-32 | 基本逐项落地；P2-25 纠正了评审前提，P2-30 改为 M0 对比，处理得当 |
| 验收补充、排期 | 资源占用、首扫目标、regex 单列、安全弹出、TCC、Windows 重启都进了 §10；Windows 进 M0/M3、加 30%–50% 缓冲进了 §11 |

## 三、M0 前必须定下来的（P0）

### 3.1 退出路径不存在，"shutdown → is_stopped"落不了地

v2 在 §4.4 承诺：退出前 `remember_size`、`shutdown`、等待有限时间完成原子保存。但基线代码里没有任何正常退出钩子：

- 托盘"退出"是 `tray.rs:125` 的 `"quit" => std::process::exit(0)`，直接从菜单回调退出进程。
- 应用内更新重启是 `update.rs:254` 的 `std::process::exit(0)`。
- `HubApp` 没有实现 `on_exit`；eframe 0.36 的 `App` trait 提供了 `on_exit(&mut self)`。

后果：

- 搜索模块的 `remember_size` / `shutdown` 永远不会被调用，窗口尺寸、面板草稿、最后一次快照保存都丢掉；按 §7.6 的保存节流，最多可能丢约 10 分钟的增量。
- `process::exit` 跳过所有析构和 eframe 清理，"有界等待"也没有执行机会。
- §4.4 的"应用退出"一行在现有代码里没有执行点；v2 的 §10.2 也恰好没有"关闭 / 退出后配置与快照落盘"这一条验收（第一版验收第 7 条），建议补回，否则这条承诺永远不会被测到。

建议：

1. 托盘退出、更新重启都改成发事件或 `ViewportCommand::Close`，让 `run_native` 正常返回；
2. `HubApp` 实现 `on_exit`：`remember_size` → `shutdown` → 有界等待（例如 2 秒，join 存储任务）；
3. 把"非阻塞 shutdown"限定为**运行中禁用**；退出场景必须允许**有界阻塞**，这两条语义在 §4.1 分开写。`is_stopped()` 的轮询只适用于禁用→重新开启的竞态，退出时事件循环已经结束，轮询不到；
4. 明写"崩溃 / 强杀不保证保存"，恢复依赖检查点 + 校验，不要在验收里承诺"退出必落盘"。

这是 1–2 天的小改动，但它决定验收能不能成立，应排在 M1 之前。

### 3.2 原生对话框必然穿透模块边界

v2 把文件夹选择划进模块（§0.1、§5.4、§8.1），但基线已经证明这件事不是"模块自己开个 NSOpenPanel"就能做完的。`platform/macos/floating.rs:346` 的保存对话框做了四件事：

- `activateIgnoringOtherApps`：菜单栏应用平时不激活，不激活就输不了文件名；
- `lower_floating_windows`：面板在 `NSPopUpMenuWindowLevel`（101，`panel.rs:226`），截屏遮罩/贴图在 1000，不临时降到普通层级，系统对话框会被自己的窗口盖住、点不到；
- `runModal`：阻塞 UI 线程；
- `DIALOG_OPEN` + 恢复层级 + 恢复前台应用，宿主 `handle_focus` 里用 `just_closed_dialog` 吃掉关闭后的首个失焦（`ui/mod.rs:577`、`:591`）。

Windows 侧同样有 `GetSaveFileNameW` + `DIALOG_OPEN`（`floating.rs:448`），并且需要 owner HWND 才能保证对话框不被置顶面板压住。

而 v2 的接口没有给模块任何操作宿主窗口的通道：

- `SearchModule::new(data_dir, ctx)` 拿不到宿主窗口（macOS 的窗口存在 `panel.rs` 的 `PANEL` 里）；
- 搜索页本身就是 101 层级、非激活的 `NSPanel`，模块的平台层无法自己降级它；
- `suppress_hide_on_blur` 是 `ui()` 的返回值，而 `runModal` / `GetOpenFileNameW` 阻塞期间不会产生新帧，这个标志最早也要等对话框关闭后才交给宿主，来不及阻止关闭后的失焦；
- §5.4"对话框存在时，热键隐藏和禁用流程先请求关闭对话框"在阻塞模态期间做不到：UI 线程就在模态循环里，`poll()` 不会运行。

这还不止文件夹选择：§5.3 的权限引导、首次授权扫描也有同样的窗口层级和激活问题。TCC 系统弹窗的层级高于普通窗口，但不是不可能被 101 层级的面板遮住，需要在 M0 实测，并准备"引导期间临时降级窗口"的兜底。

建议（按推荐顺序）：

- **A. 把 `FolderPicker` 的端口实现放到宿主平台层（推荐）**：trait 仍定义在模块的 `ports`，`SearchModule::new` 接收 `Box<dyn FolderPicker>`（或其他宿主能力集合）。宿主实现 NSOpenPanel / IFileDialog 时直接复用 `floating::save_dialog` 已经验证过的"激活应用、降级层级、抑制失焦、恢复焦点"，模块只决定何时选、初始目录、选中后如何校验和应用。**策略在模块、机制在宿主**，两边都不越界，也避免把 NSWindow / HWND 暴露成公开类型。这是"文件选择在模块内"唯一需要让渡的部分，值得在 §0.1 写明。
- **B. 更通用的对话框会话接口**：如果后续的权限引导、Quick Look 也要同一套层级保护，可以抽成 `trait DialogHost { fn begin(&self, restore_focus: bool) -> DialogGuard; }`；guard 存活期间宿主 `handle_focus` 视同 `dialog_open()`，drop 时恢复层级与焦点。接口要在 M0 就定成能覆盖文件夹选择和权限引导两种用途的形状。
- **C. 非阻塞 sheet**：macOS 用 `beginSheetModalForWindow`，Windows 用带 owner 的 IFileDialog + 事件回调。UI 线程不停顿，但 sheet 仍需要宿主窗口，通道不可避免；可以作为 B 的实现方式，而不是独立方案。
- **D. 模块自己遍历 `NSApp.windows()` 降级所有窗口**：能跑通，但这是对宿主窗口的隐式依赖，和"模块自包含"相反。不建议。

无论选哪个，`suppress_hide_on_blur` 都应该从 `SearchUiOutput` 里去掉，或降级为"非原生浮层"的开关。宿主既然持有对话框会话，就能像现在处理保存对话框一样维护 `just_closed_dialog`，比跨帧传布尔值可靠。

## 四、设计内部要先改的（P1）

### 4.1 行批次绑定 `revision` 会在持续写入时饿死 UI

§7.1 写"结果批次绑定 session_id、query_generation 和每根 `(root_id, index_epoch, revision)`，LoadRows 校验，其他过期版本要求重查"。但 §7.7 又在每次提交时递增 `revision`。在有构建、`npm install`、解压的目录里，revision 每 100–300ms 变一次：

- 查询在 revision R 返回结果 → LoadRows 到达时 R+1 → 整批拒绝 → 重查 → 又过期；
- 用户看到的是结果区反复空白或闪烁，恰好发生在最需要搜索的时候。

建议把两类失效拆开：

- **行内容**只校验 `(session, query_generation, root_id, index_epoch)` + 逐行"墓碑/排除"状态。文件还在、ID 没复用，就可以显示旧的大小/时间，下一批刷新；
- **结果集与排序**才用 revision 失效，交给 150–300ms 的防抖重查；
- 墓碑行建议直接省略或轻量标记，不要让"已删除"行闪一下再消失。

### 4.2 一个"墓碑"承担了三种语义

v2 里墓碑至少出现在三处：§6.3 新增排除规则时"在内存分批墓碑化命中子树"、§7.1 删除节点"只打墓碑、ID 不复用"、§7.7 大型子树删除"通过可见性墓碑先整体隐藏"。三者的生命周期完全不同：

- 磁盘上已删除：不可恢复，LoadRows 应拒绝或显示"已删除"；
- 被规则排除：可以恢复，解除规则后应重新可见，且不应让旧 ResultId"复活"成另一个文件；
- 批量删除中的临时隐藏：几毫秒后变成真删除。

建议拆成 `deleted` / `excluded` / `pending_remove` 三个位（或一个小枚举），并把 LoadRows 对三者的处理写进 §7.1。否则"删除规则时补扫"和"墓碑不复用"会互相打架。

### 4.3 高亮 + NFC 的偏移映射没有算成本

§7.8 要求"通过归一化到原文的映射定位，不用规范化偏移直接切原始 UTF-8"。这是对的，但映射本身有两种实现：

- 每个名字常驻一份 normalized→raw 的偏移表：内存和快照都会明显变大，且没进 §7.2 的预算；
- 只对可见行按需重算：只有几十行，代价可忽略。

建议明确选后者，并在 §7.2 注明"高亮映射不计入常驻内存"。同时写清高亮按字符区间而不是字节偏移，避免 NFD 重音字符切一半。

### 4.4 搜索页继承了浮层的窗口层级与集合行为，需要写明

v2 删掉 Normal 后，搜索页就是现有窗口：101 层级、`CanJoinAllSpaces | FullScreenAuxiliary | IgnoresCycle`、`hidesOnDeactivate=false`、无边框（`panel.rs:221-228`、`main.rs:173-177`）。对 1100×700 的搜索页意味着：

- Pinned 时窗口跟随所有桌面，并盖在全屏应用之上；
- 没有标题栏，拖动入口没定义；不能像 OCR / JSON 编辑器那样整页 `drag_window`（会抢表格手势），建议只在顶栏空白区拖动；
- 底色是玻璃（`Backdrop::Glass` 时 `clear_color` 全透明，`ui/mod.rs:680`），大表格压在桌面上可读性需要产品确认；搜索页可以自己画不透明底，不需要改平台。

这三条不一定要改代码，但要在文档里写成"v1 明确接受"，否则 M4 会当成 bug 返工。如果决定不接受，最小改动是加一个 `panel::set_search_behavior`（普通层级 / 活动桌面 / 不透明），而不是恢复整套 Normal 模式。

### 4.5 托盘入口显隐做不到，建议改成禁用态

v2 §4.2 写"根据开关显示/隐藏文件搜索"。当前 `tray-icon 0.26` 依赖的 `muda 0.21.2`，`MenuItem` 只有 `set_text` / `set_enabled`，**没有 `set_visible`**。要实现隐藏只能重建菜单（不同平台的菜单句柄和事件 id 都要重挂）。

建议：菜单项常在，关闭时 `set_enabled(false)`，文案改成"文件搜索（未启用）"。顺带解决默认关闭带来的可发现性问题——用户至少能看到这个功能存在。

### 4.6 入口收口要落到具体函数

v2 说"controller.rs 及直接 show/hide 的入口调用点统一交由宿主处理"，但没列具体位置。基线的实际情况：

- `controller::toggle_panel`（`controller.rs:48`）在主线程直接 `panel::hide(true)`；
- `controller::toggle_from_status_item`（`:79`）同样直接 `panel::hide`；
- `ui/mod.rs` 里 `self.page = ...` 有 6 处（`:329`、`:357`、`:393`、`:400`、`:492`、`:522`），另有 `panel/mod.rs:437`、`settings_page/mod.rs:214`、`ocr.rs:253`。

托盘点一下或热键在搜索页可见时，会绕过 `search.on_hidden()` 和 `remember_size`，窗口尺寸丢失、模块还认为页面可见。建议：

- `controller` 的两个入口一律 `shared.post(AppEvent::...)`，由 `HubApp::logic` 调 `hide_panel`；主线程直接调用的"优化"在这里不值得保留；
- 统一 `set_page(from, to)` 时把上面 9 处列进接入清单，`git diff` 才能按清单验收。

### 4.7 快捷面板的 FileSearch 匹配语义

`tools::actions_for` 只把 `CONTENT_ACTIONS` 交给 `matches` 过滤，`ClipboardHistory` 是兜底；`has_match` 也只看 `CONTENT_ACTIONS`（`tools/mod.rs:55-73`）。如果 FileSearch 只是"始终可用"，需要同时改 `actions_for` 和 `has_match`，否则会出现：

- 选中一段普通文本时，面板说"没有能处理的功能"，但搜索明明可用；
- 选中内容后 FileSearch 的排序位置不明确。

另外，选中的文本可能是一段代码或 URL，直接作为查询会命中 `|`、`!`、`(` 等布尔语法。建议 `SearchOptions` 增加 `raw_text: bool`（或由模块把入口文本包成短语），并写进 v2 §4.5。

### 4.8 主模块接入清单还缺几项

以下都是编译期就会撞到的，补进 §4.2 即可：

- `hotkey_errors: [Option<String>; 3]`（`ui/mod.rs:119`）与 `HotkeyAction::ALL: [HotkeyAction; 3]`（`hotkey.rs:30`）——新增第 4 个动作必须同步改长度，`HotkeyAction::index()` 依赖枚举顺序；
- `draw()` 里 `inner_margin` 与 `toast_ui` 的页面分支（`ui/mod.rs:696`、`:612`）需要 `Page::Search`；
- `handle_focus` 的 `auto_hide` 分支（`ui/mod.rs:578`）；
- 启动时 `HotkeyAction::ALL` 会遍历注册，Search 要按开关跳过（v2 已写"关闭时 suspend"，接入时保留这一条，不要只依赖空 spec）。

## 五、可以留到 M0 的（P2）

| 位置 | 问题 | 建议 |
| --- | --- | --- |
| §7.2 | 原始 + NFC + lowercase 三份文本 + arena 偏移 + 倒排，1M 条目 ≤25MB 很难成立（50 万唯一名、平均 12 字符已经 22–26MB，还没算 CJK 和目录表） | 同类文本（CJK 下 NFC 与 raw 完全相同）共享一块 arena + 标志位；M0 先量再改预算 |
| §7.4 | 默认只排除 `~/Library` 的 Caches / Containers，仍会索引 Application Support、Group Containers、Logs 等；Windows 只排 Temp 和部分缓存 | M0 用真实 `$HOME` 统计各二级目录的节点数/拒绝数，再决定整体排除 `~/Library` 还是白名单；1M / 120s 的目标要绑定数据集 |
| §7.3 | "跨卷挂载点默认不递归"没给实现方式，若每个目录额外 `statfs` 会让扫描成本翻倍 | 用目录 `lstat` 的 `st_dev`（macOS）/ reparse tag（Windows）判断，随元数据一起取 |
| §6.1 | `version: 2` + "若发现原方案 v1 配置则迁移"——如果 v1 从未发布，迁移是死代码 | 从 1 开始，或写清 v1 的识别方式与来源 |
| §6.1 / §7.5 | `RootLocation` 既被 settings 存储又被 core 使用，归属没定 | 放 core，settings 单向依赖 core 的纯数据类型；写进分层白名单，避免变成 core→settings |
| §4.3 | 缓存按 `instance_key` 隔离，但换 `--data-dir` 后旧实例目录会一直残留 | 增加"清理 N 天未使用的实例缓存" |
| §5.1（缺） | v1 的图标策略（只请求可见行、512 上限、按扩展名/目录/路径分组、占位文件用扩展图标）在 v2 丢了 | 补回 §5.1 或 `ui/icons.rs`，别留到 M4 临场决定 |
| §7.1 | 墓碑只增不减，长期运行（构建目录反复创建/删除）内存只涨不落 | 墓碑占比超过约 20% 或长时间空闲时压缩并换 `index_epoch` |
| §4.1 | `poll()` 与 `ui()` 都返回 `SearchUiOutput`，宿主怎么合并没写；非搜索页时 `request_hide` 应被忽略 | 规定优先级（`ui()` 优先、`poll()` 只发状态），或让 `poll()` 只返回策略/状态 |
| §4.1 | `SearchOptions` 同时有 `folder` 和 `selected_paths`，语义重叠 | 只留 `selected_paths`，或写明 `selected_paths` 覆盖 `folder` |
| §7.8 | `infolder:` 指向未配置索引的目录、`size:` 的 KB 是 1000 还是 1024、`type:picture` 的映射表都没写 | 分别规定"空结果 + 提示"、单位表、类型组表 |
| §5.1（缺） | egui 已启用 accesskit，自绘表格若不给行/列命名，读屏拿不到结果 | v1 不阻塞，M4 留一行 |
| §11 | M0 4–6 天要覆盖双平台元数据对比、RSS、查询/表格基准、Windows 溢出/安全弹出、TCC/签名/对话框、快照复制 | 拆成 M0a（结构与性能）和 M0b（系统集成），或明确两人并行 |

## 六、对 §12 和排期的补充

§12 的决策表已经比较完整，建议再补五行，并把一项从"验证"挪到"先定接口"：

| 事项 | 建议 |
| --- | --- |
| 退出生命周期 | 不是验证项：先改基线（托盘退出、更新重启、`on_exit`），否则 §4.4 的退出协议没有执行点 |
| 原生对话框会话 | 不是验证项：M0 前定接口（宿主能力或非模态方案），M0 只验证窗口层级与焦点恢复 |
| 搜索页窗口行为 | 明确接受 101 层级 + 全桌面 + 玻璃，或加最小平台开关；Pinned 的语义要写清 |
| 托盘入口 | 关闭时禁用而非隐藏（muda 无 `set_visible`），同时解决可发现性 |
| 默认排除 | 用真实主目录数据决定 `~/Library` / `AppData` 的排除范围，而不是先写 1M/120s 再倒推 |

排期上还有两点：

- 35–57 人天的估算比第一版可信，但 M0 的"双平台 + 系统集成"仍然偏紧，建议把 M0 拆开评估；M3 的两个平台并行也要按两个人算，而不是一个人串行。
- 若进度吃紧，v1 里最先可以砍的是：保存搜索、历史、创建时间列、图标。索引 + 查询 + 监听 + 打开/显示是主干，其余都是可后置的体验项；这比压缩测试和验收更安全。

## 七、建议顺序

1. **先改基线（1–2 天）**：托盘/更新重启的正常退出 + `on_exit`；把 `floating::save_dialog` 里的"激活应用 / 降级层级 / 抑制失焦 / 恢复焦点"抽成宿主的通用对话框会话能力。这两项不涉及搜索业务，但决定搜索模块的公开 API，越早越好。
2. **再定文档（半天）**：窗口行为（§4.4）、`DialogHost` 接口（§3.2）、`LoadRows`/墓碑语义（§4.1、§4.2）、默认排除与 version/`RootLocation`（§5）。
3. **然后进 M0**：按 M0a（结构、RSS、查询、表格、快照复制）和 M0b（TCC、签名、对话框、设备消息、安全弹出、IME）并行推进，用实测回填 §7.2、§10.3 的数字。

总体看，第二版已经把"能不能做"回答清楚了；剩下的是"和现有窗口/生命周期怎么接"。这两处接好，后面的实现就是按部就班。
