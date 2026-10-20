# Snipaste 截图与贴图实现分析

分析对象：本机 `/Applications/Snipaste.app`，版本 2.10.5（`CFBundleShortVersionString`），通用二进制（x86_64 + arm64），
`Developer ID Application: Le Liu (NGTL73P583)` 签名并已公证（`codesign -dvv`：`Notarization Ticket=stapled`）。只看 arm64 版本。

分析方法只用了静态手段加本机日志：读 `Info.plist`、签名、`otool -L`（链接库）、`nm -u`（导入符号）、`strings`（RTTI 类名、Qt moc 元数据、设置项名称）、
`otool -tV`（只反汇编了下面提到的几个函数）、解析 `__DATA_CONST,__cfstring`（otool 显示为 “bad cfstring ref” 的 CFString 常量）、
中文语言包 `Resources/lang/zh_cn.qm` 中的文案，以及 Snipaste 自己的日志 `~/.snipaste/splog.txt`。文中地址都是 arm64 切片里的虚拟地址。

## 1. 总体形态

| 结论 | 证据 |
| --- | --- |
| Qt 6.2.4 的 C++ 应用（Widgets） | `otool -L`：`@rpath/QtWidgets.framework`、`QtGui`、`QtOpenGL`、`QtSvg` 等，`current version 6.2.4`；`Contents/PlugIns/platforms/libqcocoa.dylib` |
| 菜单栏常驻，没有 Dock 图标 | `Info.plist`：`LSUIElement = 1`；导入 `NSStatusBar`、`NSStatusBarButton` |
| 原生部分用 Cocoa / Carbon / CoreGraphics / AX | `otool -L` 链接 `Cocoa`、`Carbon`、`ApplicationServices`、`CoreImage`、`ImageIO`、`Metal`；`nm -u` 见下文 |
| 截图、贴图分别是两个类 | RTTI 名称（`strings` 中 `数字+类名` 形式）：`7Snipper`、`6Paster`、`16PasterController`、`13PasterOptions`，平台相关的 `17ScreenGrabber_mac`、`18WindowDetector_mac`、`18VirtualDesktop_mac`、`22ThreadedWindowDetector` |
| 标注是一套 QGraphicsScene | `9PaintView`、`10PaintScene`、`14ArrowPaintItem`、`20RectEllipsePaintItem`、`13TextPaintItem`、`15MosaicPaintItem`、`18OpenGLMosaicEffect`、`16OpenGLBlurEffect`，撤销用 `10AddCommand` / `13ModifyCommand` |
| 全局快捷键 | `17QxtGlobalShortcut`、`24QxtGlobalShortcutPrivate`；导入 `RegisterEventHotKey`、`InstallEventHandler`、`GetEventParameter`（Carbon 热键，不需要权限） |

## 2. 整体流程（来自本机日志）

`~/.snipaste/splog.txt` 记录了一次完整的截屏（省略时间戳）：

```
Parsing command arguments [3]: (sp, snip)        ← 快捷键 / 命令行触发
Snip request: 0 (dest: 0)
Snipper: preparing...
Desktop: QRect(0, 0, 1920 x 1080)                ← 虚拟桌面范围
Background size: QSize(1920, 1080)               ← 截下来的整屏图像
Screens: 1
    QRect(0, 0, 1920 x 1080), NAME = "", DPI = 72 , DPR = 1
Snipper: preparation done (0)
Snipper: about to appear... / prepare to show... / shown
Snipper: focus in                                 ← 遮罩窗口拿到焦点
Snipper: ready [0.136 s]
Screenshot (892, 461, 586 x 290) destination: none
Snipper: about to quit... / focus out / hide / hidden / quit
```

其他记录里的 `destination: pin,`、`destination: clipboard,` 对应“贴图”和“复制”。启动时还有 `About to restore pasters...`、`Pasters restored`：
贴图会保存在 `~/.snipaste/history` 里，重启后恢复。

也就是说：**先把整个桌面截成一张静止的图，再用一个铺满屏幕的窗口显示它**，在这张图上选区、标注，最后把选区裁出来输出。
用户看到的“冻结的屏幕”就是这张图，所以截屏期间屏幕内容不会变化，鼠标悬停效果、动画也不会被截进去。

## 3. 截屏：`CGDisplayCreateImageForRect` 逐个显示器截图再拼接

导入的相关符号（`nm -u`）：`_CGGetActiveDisplayList`、`_CGGetDisplaysWithRect`、`_CGDisplayBounds`、`_CGDisplayCreateImageForRect`、
`_CGImageCreateCopyWithColorSpace`、`_CGColorSpaceCreateWithName`、`_kCGColorSpaceSRGB`、`_CGRectUnion`、`_CGRectIntersection`、`_CGRectOffset`。
没有导入 ScreenCaptureKit（`SCShareableContent`、`SCScreenshotManager`），也没有 `CGWindowListCreateImage`。

反汇编截屏函数（约 0x10032d120 起，`ScreenGrabber_mac`）：

| 地址 | 调用 | 作用 |
| --- | --- | --- |
| 0x10032d198 | `CGGetActiveDisplayList` | 取所有显示器 |
| 0x10032d300 | `CGGetDisplaysWithRect` | 取与请求区域相交的显示器 |
| 0x10032d3a0 / 0x10032d3c4 | `CGDisplayBounds` → `CGRectUnion` | 合并成整个虚拟桌面（日志里的 `Desktop: QRect(...)`） |
| 0x10032d454 / 0x10032d460 | `QImage(w, h, Format 6)`、`fill(0x13)` | 新建 `Format_ARGB32_Premultiplied` 画布并填成 `Qt::transparent`（显示器之间的空隙透明） |
| 0x10032d4b8 | `CGColorSpaceCreateWithName(kCGColorSpaceSRGB)` | 准备 sRGB 色彩空间 |
| 0x10032d524–0x10032d5c0 | `CGDisplayBounds` → `CGRectIntersection` → `CGRectOffset` | 每个显示器与请求区域的交集，换成显示器本地坐标 |
| **0x10032d5c8** | **`CGDisplayCreateImageForRect`** | 截取这一块 |
| 0x10032d5d4 | `CGImageCreateCopyWithColorSpace` | 把图像**标记**为 sRGB（只换标记、不转换数值） |
| 0x10032d5e0 / 0x10032d644 | `qt_mac_toQImage` → `QPainter::drawImage` | 转成 QImage 画到画布上对应的位置 |
| 0x10032d6b0–0x10032d700 | `setDotsPerMeterX/Y`、`QPixmap::fromImage`、`setDevicePixelRatio` | 记下 DPI 与缩放比例（Retina 屏截图是物理像素） |

“只换色彩空间标记”意味着取色器读到的就是屏幕上显示的原始数值，不受显示器色彩配置（如 Display P3）转换的影响。

权限：截屏需要“屏幕录制”权限。CFString 常量里有
`x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture`（0x100561ed8，打开系统设置对应页面）
和 `Privacy_Accessibility`（0x100561e78）。

## 4. 自动识别窗口与界面元素

### 4.1 窗口：`CGWindowListCreate` + `CGWindowListCreateDescriptionFromArray`

窗口检测函数（约 0x10032e174 起，`WindowDetector_mac`）：

- 0x10032e18c `getpid()`，再 `+[NSRunningApplication runningApplicationWithProcessIdentifier:]`：取本进程（推测用来排除自己的窗口，如贴图）；
- 0x10032e1ec–0x10032e1fc：`CGWindowListCreate(1, 0)` → `CGWindowListCreateDescriptionFromArray`，第一个参数 1 是 `kCGWindowListOptionOnScreenOnly`；
- 之后遍历每个窗口的字典，读取 `kCGWindowAlpha`、`kCGWindowIsOnscreen`、`kCGWindowBounds`、`kCGWindowLayer`、`kCGWindowName`、`kCGWindowOwnerPID`（0x10032e230–0x10032e27c 依次加载这些键）；
- 0x10032e2e4 `CFNumberGetValue(alpha, kCFNumberFloat32Type = 5)`，0x10032e2f0 `fcmp s0, #0.0`：**透明度为 0 的窗口跳过**；`kCGWindowIsOnscreen` 为假的跳过；
- 0x10032e34c `CGRectMakeWithDictionaryRepresentation` 取出窗口位置。

`CGWindowList` 返回的顺序就是从上到下的层叠顺序，鼠标下第一个包含它的窗口就是要框出的窗口。
Snipper 的 moc 元数据里有 `requestRectDetection(screenPos, deepScan, excludeFullWins)`、`handleDetectedRect(requestId)`、`onWindowDetectorReady`，
`ThreadedWindowDetector` 有 `queryRectQueued`、`queryParentRect`、`queryChildRect`、`consumeQueries`：检测放在后台线程，按请求编号返回结果，
并且可以排除全屏窗口（`excludeFullWins`）。设置项里对应 `detect_windows`。

### 4.2 界面元素：辅助功能 API

导入 `_AXUIElementCopyElementAtPosition`、`_AXUIElementCreateApplication`、`_AXUIElementCopyAttributeValue`、`_AXValueGetValue`、`_AXIsProcessTrustedWithOptions`：

- 0x10032de60–0x10032e0dc：`AXUIElementCreateApplication(pid)` → 读 `AXWindows`（CFString 0x100561c18）→ 对每个窗口读 `AXPosition`（0x100561c58）、`AXSize`（0x100561c78）；
- 0x10032ec08：`AXUIElementCopyElementAtPosition` 取鼠标下的元素，0x10032ed38 再读 `AXChildren`（0x100561c98）逐层往下找，配合 `QRect::contains` 判断；
- 0x10032e5e4 附近与字符串 `"Notification Center"` 比较；CFString 里还有 `com.apple.dock`、`Dock`：对通知中心、程序坞做了特殊处理。

对应设置项 `detect_ui_elements`，中文快捷键说明“切换检测窗口/检测界面元素”“选择上一层/下一层界面元素”。

## 5. 截屏遮罩（`Snipper`）的交互

Snipper 的 moc 元数据（信号、槽和属性名）与设置项（`strings` 中 `7Snipper` 前后）：

- 选区：`setSnipRect`、`resizeSnipRect`、`checkRatio`、`setAspectRatio`、`updateResizeType`、`reviseCursorPosForResizing`、`showBorders`、`updateMarkersPos`（八个控制点）；
- 放大镜与取色：`updateColorPicker`、`show_mask_in_magnifier`、`show_border_in_magnifier`；
- 辅助线：`setGuideLines`、`showCrossLines`、`auto_cross_lines`；
- 输出：`output(dest)`、`outputByShortcut`、`enter_key_output`、`double_click_output`、`middle_click_output`、`quit_after_enter_key`；
- 其他：`capture_cursor`、`ask_for_confirm_on_esc`、`loop_history_areas`、`max_records`（截屏历史）、`startDelayed`（延时截屏）、`startWhiteboard`。

界面文案（`zh_cn.qm`）给出了默认操作：

| 操作 | 文案 |
| --- | --- |
| 输出 | `Pin to screen (Ctrl+T)`、`Save to file (Ctrl+S)`、`Copy to clipboard (Ctrl+C)`（英文原文，在 `strings` 中相邻）；“复制截图并退出截屏” |
| 右键 | “重新设置截屏区域/中止截屏” |
| 方向键 | “将截屏区域移动 1 像素”“将截屏区域相应边界扩大 / 缩小 1 像素” |
| 放大镜 | “放大镜可见时，复制颜色值”“放大镜可见时，切换颜色格式” |
| 其他 | “使用上一次截屏的区域”“设置截屏区域为当前屏幕/全屏”“截屏时显示/隐藏辅助线” |

## 6. 贴图（`Paster`）

### 6.1 窗口：`QNSPanel`，替换 `setLevel:` 保持置顶

贴图是 Qt 的 `Qt::Tool` 窗口，在 macOS 上就是 `NSPanel`（Qt 的类名 `QNSPanel`）。Snipaste 在启动时用运行时 API 改了 Qt 的窗口类
（函数 0x10038e4b8；CFString 0x100561cd8 = `QNSView`、0x100561cf8 = `QNSPanel`、0x100561d18 = `QNSStatusItem`）：

```
NSClassFromString(@"QNSView")   → class_addMethod(windowLevel, imp 0x10038e588, "l@:")
NSClassFromString(@"QNSPanel")  → class_getInstanceMethod(setLevel:) → method_setImplementation(imp 0x10038e5ec)
NSClassFromString(@"QNSStatusItem") → 替换 item
NSStatusBarButton: method_exchangeImplementations(sendActionOn:, patched_sendActionOn:)
```

- 新的 `setLevel:`（0x10038e5ec）：`tst x21, #0xfffffffffffffff7` —— 层级不是 0（`NSNormalWindowLevel`）或 8（`NSModalPanelWindowLevel`）时调用原实现；
  是 0 或 8 时走自己的逻辑。推测是拦住 Qt 在窗口状态变化时把层级改回普通层级的调用，让贴图一直浮在其他窗口之上；
- 给 `QNSView` 加的 `windowLevel`（0x10038e588）：返回所在窗口的层级，没有窗口时返回 3（`NSFloatingWindowLevel`）；
- `patched_sendActionOn:`（0x10038e49c）：把掩码 `0x04000014`（左 / 右 / 其他键松开）换成 `0x0200000a`（按下），菜单栏图标在按下时就响应。

窗口相关的选择器（`__objc_methname`）：`setLevel:`、`setCollectionBehavior:`、`setIgnoresMouseEvents:`（鼠标穿透）、`setHidesOnDeactivate:`、`setMovableByWindowBackground:`。

### 6.2 功能

`Paster` 的 moc 元数据：`setTopmost`、`showShadow`、`deltaZoom(delta, std::optional<QPointF> anchor)`、`setZoom`、`zoomBySteps`、
`deltaImageOpacity`、`rotateImage`、`setMirroring`、`setInputTransparency` / `toggleInputTransparency`、`copyText`、`replaceByClipboard`、
`replaceByOpenFile`、`exportSelected`、`setCornerRadius`、`disableActivationForNextShow`、`doMouseAction`。

`PasterOptions` 的设置项：`zoom_at_cursor`（以鼠标为中心缩放）、`wheel_for_zoom` / `key_for_zoom`、`wheel_for_opacity` / `key_for_opacity`、
`mouse_action_for_close` / `_for_reset` / `_for_thumbnail` / `_for_text_copy`、`esc_action`、`shift_esc_action`、`thumbnail_width/height`、
`max_window_size`、`activate_on_pasted`、`text_to_image`（文本贴图）、`shadow_color`。全局设置里还有 `show_shadow`、`default_opacity`、
`smooth_scaling`、`pin_over_desktops`、`auto_backup`。

文案：“双击贴图窗口会使其关闭。你可以在首选项窗口中（控制 - 鼠标）更改此行为。”、“水平翻转贴图”“垂直翻转贴图”
“将贴图顺时针旋转 90 度”“开/关鼠标穿透”“以剪贴板中的内容替换该贴图”“隐藏/显示所有贴图”“销毁贴图窗口”。
设置页的全局快捷键列表里“截屏”“贴图”是两个独立的快捷键（`snip`、`paste` 也是命令行参数，见第 2 节日志）；Snipaste 文档中 Windows 版的默认值是 F1 截屏、F3 贴图。

## 7. 在本项目中的实现

| Snipaste | 本项目（Rust + egui） |
| --- | --- |
| `ScreenGrabber_mac`：`CGDisplayCreateImageForRect` + `CGImageCreateCopyWithColorSpace(sRGB)` | `platform/macos/screen.rs::capture`：同样的调用，画进 RGBA 位图。放在后台线程执行：macOS 15 起这个接口经 ScreenCaptureKit 代理，系统确认权限时会卡住好几秒（实测新编译、未获准的程序最后返回空），期间界面照常响应；失败时打开设置页的“截屏与贴图”分组 |
| 截整个虚拟桌面 | 只截鼠标所在的显示器（见第 8 节） |
| `WindowDetector_mac`：`CGWindowList…`，跳过透明窗口和自己的窗口，`excludeFullWins` | `screen.rs::windows`：`CGWindowListCopyWindowInfo(OnScreenOnly \| ExcludeDesktopElements)`，跳过透明、本进程、层级为负的窗口；`snip.rs::Capture::new` 跳过覆盖屏幕 95% 以上的浮动窗口（本机实测有一个 `EnUES` 遮罩窗口是 `0, 1, 1920 × 1079`，比屏幕少一像素；Snipaste 日志里也有一次 `Screenshot (0, 1, 1920 x 1079)`） |
| 静止截图 + 铺满屏幕的窗口 | `ui/snipper.rs`：egui 延迟视口；`platform/macos/floating.rs::begin_overlay` 把层级设为 `NSScreenSaverWindowLevel` 盖住菜单栏，激活本应用以显示十字光标，结束后把焦点还给之前的应用 |
| 单击选中窗口 / 拖动框选 / 八个控制点 / 方向键 | 同样；计算在 `snip.rs`（`rect_between`、`hit`、`drag_handle`、`nudge`），有单元测试 |
| 放大镜与取色、C 复制颜色 | 15×15 像素放大 8 倍、十字线、坐标与 `#RRGGBB`，按 C 复制 |
| `Ctrl+T` 贴图、`Ctrl+S` 保存、`Ctrl+C` / 双击 / 回车复制、右键重选 / 退出、esc 退出 | 同样（macOS 上是 ⌘T、⌘S、⌘C）；保存用系统保存对话框（`NSSavePanel` / `GetSaveFileNameW`），截图复制后照常进入剪贴板历史 |
| 标注：`PaintScene` 与 `RectEllipsePaintItem`、`ArrowPaintItem`、`PencilPaintItem`、`HightlightPaintItem`、`MosaicPaintItem`、`TextPaintItem`、`EraserPaintItem`，`AddCommand` / `ModifyCommand` 撤销 | `snip/annotation.rs`（数据、橡皮擦命中测试、马赛克格子、撤销重做）+ `ui/snipper/`：工具栏与 Snipaste 相同的顺序（矩形 / 椭圆、箭头、画笔、记号笔、马赛克、文字、橡皮擦｜撤销、重做｜退出、贴图、保存、复制），选中工具后出现属性栏（矩形 / 椭圆、三档粗细或字号、八种颜色）；⌘Z / ⇧⌘Z 撤销重做 |
| 贴图的标注工具条：`Paster::setEditing`、`updatePaintBar`，快捷键说明“显示/隐藏标注工具条”（空格） | 贴图右键菜单“编辑标注”或空格：重新打开截屏遮罩，选区固定为贴图的范围、用贴图原图作底；完成（贴图按钮）、复制、保存时把带标注的图片写回贴图，退出则放弃。没画标注时输出原图，分辨率不变 |
| 输出带标注的图片 | 有标注时进入“导出”：遮罩只画截图和标注，用 `ViewportCommand::Screenshot` 截下这一帧再裁出选区（文字、抗锯齿与屏幕上一致）；没有标注时直接裁原图。实测导出的像素与屏幕完全相同 |
| `Paster`：`QNSPanel` + 替换 `setLevel:` 保持置顶 | `ui/pin.rs`：每张贴图一个 egui 延迟视口；`floating::configure_pin` 换成不激活应用的 `NSPanel` 子类，层级 `NSFloatingWindowLevel`，点击、拖动贴图不抢前台应用的焦点 |
| 拖动、`deltaZoom` 以鼠标为中心、`deltaImageOpacity`、`rotateImage`、`setMirroring`、`setTopmost`、`showShadow` | 拖动移动；滚轮 / 触控板捏合缩放（以鼠标为中心，`snip::zoom_about`）；⌘ + 滚轮调整不透明度（`NSWindow.alphaValue`）；R / ⇧R 旋转；右键菜单里翻转、缩放、不透明度、置顶、阴影 |
| 双击 / esc 关闭，右键菜单 | 同样；菜单是原生 `NSMenu`（Windows 上是项目里的自绘菜单），另有“关闭所有贴图” |
| 托盘“截屏”“贴图”，全局快捷键 `QxtGlobalShortcut`（`RegisterEventHotKey`） | 托盘菜单加了“截屏”“贴图（剪贴板中的图片）”“关闭所有贴图”；`hotkey.rs` 支持多个快捷键，默认 macOS ⌃⇧A / ⌃⇧V、Windows F1 / F3，可在设置页录制 |
| 命令行 `snip`、`paste` | 启动参数 `--snip`、`--pin <图片>`（只在启动时生效） |
| “屏幕录制”权限链接 `Privacy_ScreenCapture` | `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess`，设置页显示授权状态，同一个链接打开系统设置 |
| — | 剪贴板历史面板里的图片条目可以“贴到屏幕” |

Windows 上的对应实现：`platform/windows/screen.rs` 用 GDI `BitBlt`（带 `CAPTUREBLT`）截屏，`EnumWindows` + `DWMWA_EXTENDED_FRAME_BOUNDS` 取窗口位置
（跳过隐藏、最小化、cloaked、鼠标穿透和本进程的窗口）；`platform/windows/floating.rs` 按物理像素摆放截屏遮罩。

### egui 多视口的几处坑

截屏遮罩和贴图都是 egui 的子视口，只能在主界面的 `ui` 里创建：

- 面板隐藏时 eframe 只调用 `logic`、不运行界面（`update_logic_only`），快捷键触发时就没有机会创建窗口。
  `platform::panel::wake` 把面板窗口以全透明、不接收鼠标的方式临时摆出来，让下一帧运行界面；
  子视口创建出来后由 `finish_wake` 收起 —— 之后只要有子视口可见，eframe 即使面板隐藏也会运行界面（`is_viewport_or_descendant_visible`）。
- 子视口必须用**延迟视口**（`show_viewport_deferred`）。即时视口（`show_viewport_immediate`）的窗口在 `ui` 里同步创建，
  要用 eframe 线程局部的“当前事件循环”；面板隐藏时 eframe 从 `check_redraw_requests`（winit 的 `new_events`）直接运行界面，
  那里没有设置它，窗口建不出来，egui 随即 panic：“egui backend is implemented incorrectly - the user callback was never called”。
  延迟视口的窗口由 eframe 在 `initialized_all_windows(event_loop)` 里创建，不受影响。

- 主界面以外的窗口里调用 `Context::request_repaint()` 只会重绘那个窗口。后台线程、贴图窗口发给主界面的事件都要用
  `request_repaint_of(ViewportId::ROOT)` 唤醒主界面，否则贴图的右键请求要等到别的事件才会被处理。
- 子视口结束前的最后几帧也要照常绘制：回调里什么都不画，这一帧就是空白的，全屏的截屏遮罩会闪一下、贴图会变成一块灰色。

另外，贴图与面板一样把 winit 的窗口换成 `NSPanel` 子类（`object_setClass`）。winit 创建窗口时用 KVO 观察 `effectiveAppearance`，
KVO 已经把实例的类换成了动态子类 `NSKVONotifying_WinitWindow`，直接换类会让这条登记失效，窗口销毁时
（关闭贴图）winit 移除观察者就抛出 `NSRangeException` 崩溃。`panel::make_nonactivating` 在换类前移除观察者、换类后按 winit 的参数重新登记。

## 8. 没有实现或做法不同的部分

- **标注**：实现了矩形 / 椭圆、箭头、画笔、记号笔、马赛克（笔刷）、文字、橡皮擦和撤销重做；没有实现序号（`NumberPaintItem`）、
  折线、放大镜标注（`MagnifyPaintItem`）、模糊，也不能在画好之后选中标注再移动、修改（Snipaste 可以）；
- **界面元素识别**（AX 逐层查找子元素）、截屏历史（`max_records`、回溯上次区域）、延时截屏、白板模式、辅助线、截取鼠标指针：没有实现；
- **跨显示器截屏**：Snipaste 截整个虚拟桌面、一个窗口跨多个显示器；这里只截鼠标所在的显示器（macOS 默认“显示器具有单独的空间”，一个窗口跨不了多个显示器）；
- **贴图**：没有鼠标穿透、缩略图模式、文本转图片贴图、分组、重启后恢复（Snipaste 存在 `~/.snipaste/history`）；
- **Windows 上不支持调整贴图不透明度**：分层窗口（`WS_EX_LAYERED`）与 wgpu 的 DXGI 翻转模型交换链不兼容；
- 命令行参数只在启动时生效，没有像 Snipaste 那样把参数转发给已经在运行的实例（`SingleApplication`）。

## 9. 复现方法

```bash
BIN=/Applications/Snipaste.app/Contents/MacOS/Snipaste
# 导入的系统符号（截屏、窗口列表、AX、热键）
nm -u -arch arm64 $BIN | grep -E '^_(CG|AX|Register|Install)'
# RTTI 类名与 moc 元数据
strings -a -arch arm64 $BIN | grep -n -E '^[0-9]+(Snipper|Paster|ScreenGrabber_mac|WindowDetector_mac)$'
# 反汇编，定位截屏与窗口检测的调用点
otool -arch arm64 -tV $BIN > dis.txt
grep -n -E 'CGDisplayCreateImageForRect|CGWindowListCreate|AXUIElementCopyElementAtPosition|selector ref: setLevel:' dis.txt
# 运行时日志
grep -E 'Snipper|Screenshot|Paster' ~/.snipaste/splog.txt | tail -30
```

CFString 常量：`otool -l` 找到 `__DATA_CONST,__cfstring`（arm64 切片中位于 0x100561c18，大小 0x300），每项 32 字节
（isa、flags、字符串指针、长度），字符串指针去掉高位的链式修正位后指向 `__cstring`。中文文案：`.qm` 文件中翻译是 `0x03` + 4 字节长度 + UTF-16BE 字符串。
