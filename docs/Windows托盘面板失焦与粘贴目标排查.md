# Windows 托盘面板：失焦不隐藏、粘贴目标错误的排查记录

## 现象

两个现象都只在 Windows 的**托盘点击**路径上出现，macOS 正常：

1. **失焦不自动隐藏**：点托盘图标弹出面板后，切到别的程序，面板留在屏幕上不关；macOS 上会关。
2. **粘贴目标错误**：菜单里"粘贴到 X"显示的 X 是任务栏（Windows 资源管理器）或 devtools-hub 自己，自动粘贴（Ctrl+V）也就发给了错误的窗口。

## 结论

两个现象的前提是同一个：**托盘点击这条路径上，面板成为前台窗口的方式很特殊**。

1. 失焦不隐藏是**两层**原因叠加，缺一不可：焦点状态信号本身不可靠，**并且**失焦不会产生重绘事件，判断代码根本没机会执行。
2. 粘贴目标错误是：弹出面板那一刻的前台窗口是任务栏或我们自己的托盘窗口，它被当成了"之前的程序"。

修复：

- `ui/mod.rs` 的 `handle_focus` 在 Windows 上改用 `native_panel::is_active()`（即 `IsWindowVisible(hwnd) && GetForegroundWindow() == hwnd`）判断焦点，不再依赖 winit 的 `viewport().focused`。
- 面板显示期间每 200 ms 排一次重绘（`request_repaint_after`），保证焦点变化一定会被 `handle_focus` 看到；面板隐藏时不排重绘，保持空闲。
- 剪贴板监听线程每轮（500 ms）采样一次前台窗口，记住最近一个"别的程序"的窗口（`workspace::remember_foreground`）；弹出面板时如果当前前台不是别的程序（任务栏 / 托盘窗口），回退到它（`workspace::last_other_foreground`）。

## 关键机制

### 为什么 winit 的焦点状态在托盘路径上不可靠

- eframe 每帧通过 `egui_winit::update_viewport_info` 取 `Some(window.has_focus())` 填 `ViewportInfo::focused`。
- winit 在 Windows 上 `has_focus()` = `is_active && is_focused`：
  - `is_active` 只由 `WM_NCACTIVATE` 置位；
  - `is_focused` 由 `WM_SETFOCUS` / `WM_KILLFOCUS` 置位。
- `panel::bring_to_front` 用 `AttachThreadInput` + `SetForegroundWindow` + `SetFocus` 把面板提到前台，然后**detach** 输入队列。detach 会撤销键盘焦点 → `is_focused` 变回 false → `has_focus()` 恒为 false，**即使面板已经是前台窗口**。

所以"是否失去焦点"这件事，Windows 上必须问系统（`GetForegroundWindow`），不能问 winit。

### 为什么只改信号还不够，必须加轮询

winit 只在状态**变化**时发 `WindowEvent::Focused`。面板弹出时 `is_focused` 已经因为 detach 变成 false 了；用户再切到别的程序时，winit 认为状态没变化，**不发事件**。没有事件 → eframe 不重绘 → `App::ui` 不运行 → `handle_focus` 一次都不执行。表现就是"信号错了"和"代码没跑"叠加，只修前者仍然复现（实测：只改信号，6 次托盘点击里仍有 5 次不隐藏）。

## 诊断方法

可复现的最小实验：

1. 用**真实的托盘点击**驱动：`Shell_NotifyIconGetRect` 拿图标 rect（本应用 uID=2），`SetCursorPos` 移到图标中心，`mouse_event` 发左键按下/抬起。`--show` 和热键路径**复现不出来**，只有托盘路径有这个问题。
2. 观察点要问系统，不要只看日志：`GetGUIThreadInfo` 的 `hwndActive` / `hwndFocus` 分别对应 winit 的 `is_active` / `is_focused`；再用 `GetForegroundWindow` 和 `IsWindowVisible`。
3. 面板窗口用 `FindWindowW("Window Class", "DevTools Hub")` 找（winit 的窗口类名就是 `Window Class`，标题是应用名）。

## 踩过的坑

| 坑 | 表现 | 处理 |
| --- | --- | --- |
| release 二进制过期 | 日志里出现源码中不存在的文本，据此得出的结论全部无效 | 每次实验前重新 `cargo build --release -p devtools-hub` |
| `Process.MainWindowHandle` 按 PID 缓存 | 探针第一次读到"可见"之后再不变，把真实发生过的隐藏掩盖掉，差点误判为"修复失效" | 改用无缓存的 `FindWindowW` + `IsWindowVisible` 查询 |
| PowerShell 非 DPI 感知 | `Shell_NotifyIconGetRect` 返回缩放后的 rect，点击落不到图标上 | 先调用 `SetProcessDPIAware()` |
| `FindWindowW` 的类名传 `$null` | 直接返回 0 | 传具体类名；或传 `[NullString]::Value` |
| 用 `AppActivate` 代替真实点击 | 复现不出来 | 必须 `SetCursorPos` + `mouse_event` |
| Git Bash 的 grep / sed / cat -A 会吃掉 CR | 行尾检查结果不可信 | 用 `git ls-files --eol`，或 perl 以 `:raw` 处理 |
| Windows 的 `panel::hide` 没有日志 | "日志里没有隐藏"被当成"没有隐藏" | 用外部可见性探针判断，不要靠日志文本 |
| Esc / Enter 需要键盘焦点 | 合成按键到不了面板（托盘路径下键盘焦点可能不在面板上） | 用全局热键或鼠标事件驱动 |
| `iconv ... \|\| powershell ...` 写在一行 | iconv 失败时整个实验会被跑第二遍，数据全乱 | 先落盘，再单独转换 |
| 一个 PowerShell 脚本里做多件事 | 失败时不知道哪一步出的错 | 每步打带时间戳的标记，先写操作步骤再写应用日志 |

## 验证数据

修复前后都用真实的托盘点击驱动，观察面板可见性与前台窗口：

| 场景 | 修复前 | 修复后 |
| --- | --- | --- |
| 托盘点击弹出后切到别的程序 | 6 次里 5 次不隐藏 | 6/6 自动隐藏 |
| 热键弹出后切到别的程序 | — | 2/2 自动隐藏 |
| 托盘点击开关面板 | — | 3/3 正常 |
| `之前的前台程序` | `Some("Windows 资源管理器")` 或 `Some("devtools-hub")` | 真实程序名（实测 `Some("Notepad++")`、`Some("Windows PowerShell")`） |

复核时（外部探针，无缓存）实测到的完整链条：

- 点托盘 → 面板成为前台窗口（`devtools-hub[Window Class]`，可见=是）
- 按热键关面板 → 前台回到弹出前的程序（接收窗口的进程），说明 `hide(true)` 的 `SetForegroundWindow(PREVIOUS)` 确实落在了正确的窗口上
- 面板显示时切到别的程序 → 面板可见=否，且前台是那个程序（没有把焦点抢回面板）

## 未验证项

- **macOS 未验证**：本机没有 Apple 工具链，无法构建或运行。macOS 侧这次只动了共享代码（面板显示期间排重绘），`handle_focus` 在 macOS 上仍走 `viewport().focused`，行为与改动前一致。
- 托盘点击关闭面板（不是粘贴）时，前台可能仍停在任务栏：`hide(true)` 用的是普通 `SetForegroundWindow`，而托盘点击会让任务栏成为前台窗口，可能被前台锁拦下（`bring_to_front` 有 `AttachThreadInput` 绕过，`hide` 没有）。这是既有实现，与本次两个修复无关，粘贴路径不受影响（粘贴时面板自己就是前台窗口，调用是允许的）。

## 教训

- **跨平台抽象层的状态不等于系统状态**。涉及前台窗口、键盘焦点这类平台概念时，用平台自己的判据，并且把"库的抽象错了"当作默认假设去验证。
- **事件驱动的状态机在事件源本身出错时会静默失效**。信号不可靠时，兜底轮询不是丑代码，是必需的正确性保证。
- **同一路径上的相邻假设要一起查**。修好"失焦不隐藏"之后，同一条路径上"前一个程序是谁"的假设同样是错的，两个 bug 有同一个前提。
- **验证要能证伪**。"日志里出现了正确的程序名"不足以证明回退逻辑生效（可能前台本来就是它）；加一条只有修复后才成立的观测（焦点归还到哪个窗口）才能真正证伪。
