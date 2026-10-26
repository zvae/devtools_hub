# Windows 无边框窗口的缩放边框排查

## 现象

Windows 上面板的四个边、四个角都不出现缩放鼠标，只有窗口最顶上一条极细的位置能拖大拖小，看起来像是"只有右上角能用、另外三个角被旁边的按钮挡住了"。macOS 上一切正常。

## 测量

给面板窗口直接发 `WM_NCHITTEST`（屏幕坐标按 `GET_X_LPARAM` / `GET_Y_LPARAM` 打包进 `lParam`），再用 `GetCursorInfo` 读当前光标，另外用 `GetWindowRect` / `GetClientRect` 看客户区和窗口矩形的关系（175% 缩放，380×640 的面板）：

| 位置 | 窗口矩形 → 客户区 | 命中码 | 光标 |
| --- | --- | --- | --- |
| 左上角（左上各 1 像素） | 380×640，偏移 0 | `HTTOPLEFT` | 缩放箭头 |
| 上边中部，`y = top` | 同上 | `HTTOP` | 上下缩放 |
| 上边中部，`y = top + 1` | 同上 | `HTCLIENT` | 箭头 |
| 右上角，`y = top` | 同上 | `HTTOPRIGHT` | 缩放箭头 |
| 左 / 右 / 下边，以及三个角 | 同上 | `HTCLIENT` | 箭头 |

结论：**非客户区只剩顶部 1 个物理像素**，其余位置客户区与窗口矩形完全重合。所以能拖的只有那条一像素的横线（且只有它的左右两端才是对角缩放），另外三条边和三个角根本没有非客户区，跟旁边的按钮无关。

## 原因

1. `with_decorations(false)` 时，winit 仍然保留 `WS_SIZEBOX`（`resizable` 为真就有），所以窗口样式上看是"可以缩放"的（`platform_impl/windows/window_state.rs` 的 `to_window_styles`）。
2. 但 `WM_NCCALCSIZE` 会把客户区扩到整个窗口矩形（`event_loop.rs`）：只有当窗口带 `MARKER_UNDECORATED_SHADOW` 时才走 `params.rgrc[0].top += 1; bottom += 1` 那条分支。
3. 于是客户区覆盖了窗口的整个矩形，`DefWindowProc` 的 `WM_NCHITTEST` 对任何点都返回 `HTCLIENT`：没有非客户区，系统就不会显示缩放鼠标，也没有拖动缩放的循环。顶部那 1 像素就是上面第 2 步的偏移漏出来的。
4. macOS 的 `NSWindow` 缩放边框在 `contentView` 之外，跟有没有标题栏无关，所以 `with_decorations(false)` 不影响缩放。

eframe 打开 undecorated shadow（`egui-winit` 的 `decorations == false && !fullscreen`）也解释了为什么不是"一处都没有"：如果没有这块投影处理，连顶部那一像素都没有。

## 修改

新增 `platform/windows/resize.rs`：

- 用 `SetWindowSubclass` 接住 `WM_NCHITTEST`，用 `GetWindowRect` + `GetDpiForWindow` 算四周 5 个逻辑像素的边框，落在边框里就返回 `HTLEFT` / `HTTOPLEFT` / … 这些命中码，其余情况原样交给 `DefSubclassProc`。系统据此自己换光标、自己跑拖动缩放，和带标题栏的窗口一致。
- `IsZoomed` / `IsIconic` 时返回 `None`：最大化、最小化时不该接管。
- 在 `panel::attach` 里绑定窗口时调用一次。贴图和截屏遮罩都是 `with_resizable(false)`，不需要。

注意这条边框占的是客户区最外面几个像素：

- 顶部栏、底部栏的按钮离边缘有 `style::PAD`（8）的间距，5 像素的边框碰不到。
- 列表右侧的滚动条平时只有 2 像素宽，但它自己的交互矩形按展开后的 `bar_width`（默认 10）算，剩下的一段仍然够鼠标移上去让它展开；滚轮不受影响。

## 验证

同一套探针在修改后重跑（把窗口摆到空白处并置顶，排除被别的窗口遮住的干扰）：

| 位置 | 命中码 | 光标 |
| --- | --- | --- |
| 左上角 / 右上角 | `HTTOPLEFT` / `HTTOPRIGHT` | 对角缩放 |
| 左下角 / 右下角 | `HTBOTTOMLEFT` / `HTBOTTOMRIGHT` | 对角缩放 |
| 上 / 下边中部 | `HTTOP` / `HTBOTTOM` | 上下缩放 |
| 左 / 右边中部 | `HTLEFT` / `HTRIGHT` | 左右缩放 |
| 边框以内 12 像素、窗口中心 | `HTCLIENT` | 箭头 |

用 `mouse_event` 实际按住拖动：右下角 380×640 → 440×680，左上角 → 430×680（原点同时移动），左边 → 420×640，下边 → 380×670；右下角往里拖过头时钳在最小尺寸 240×120（`with_min_inner_size`）。

排查时发现的一个坑：探针里窗口没置顶时，四个角量出来的光标是箭头，是别的窗口盖在上面造成的，和边框逻辑无关；比对命中码才能判断对错。
