# uTools 鼠标中键面板（超级面板）实现分析

分析对象：本机 `/Applications/uTools.app`，版本 7.8.0，Electron 应用，`Developer ID Application: NaiQuan Wu (9RRPKPTRUT)` 签名并已公证（`codesign -dvv`）。只看 arm64 版本的 macOS 实现，顺带记录 Windows / Linux 分支。

分析方法只用了静态手段：读 `Info.plist`、签名、`otool -L`（链接库）、`nm`（导入与导出符号）、`otool -tV`（只反汇编了下面提到的两个函数），并解包 `Resources/app.asar`（asar 是“JSON 文件头 + 拼接的文件内容”的简单归档，用 Python 按文件头切出文件即可）阅读其中的 JavaScript。JS 都是压缩过的单行文件，下文用“文件名 + 字节偏移”标注出处，引用的代码做了少量排版（换行、缩进），标识符保持原样。

本机设置（`~/Library/Application Support/uTools/settings`）：`"enableVoicePanel": true`、`"voicePanelTriggerWay": 0`，即“鼠标按键弹出：按下鼠标中键”。

> 这个面板在界面上叫“超级面板”（设置页分组“超级面板”，开关“启用超级面板”），代码里叫 **voicePanel**（窗口加载 `dist/voice.html`），因为它同时承担语音输入和翻译。

## 1. 整体流程

```
按下中键
  │  CGEventTap（原生插件，HID 层）：按键号 == 2、前台应用不在黑名单
  │  → 回调 JS：nativeVoicePanelTrigger(1, x, y)，并返回 NULL 吞掉这次点击
  ▼
triggerShow(x, y)                                   dist/main.js
  ├─ 刚复制过（autoPasteLife 秒内）→ payload = 剪贴板内容（“复制的文本”）
  └─ 否则：记下 changeCount → 模拟 ⌘C → payload = {type: "simulatecopy"}
  │  面板窗口 showInactive() 显示在鼠标处（不抢焦点，⌘C 仍发给原来的应用）
  ▼
渲染进程 showInit(payload)                          dist/voice/index.js
  │  50 ms 后：isSimulateCopyForClipboardUpdate() = changeCount 是否变大
  ├─ 变了 → 读剪贴板 → 按插件声明的规则匹配功能 → 标题“选中的文本 389 个”
  └─ 没变 → 按当前窗口匹配功能 / 显示固定在面板上的常用功能
  ▼
松开中键（code 3）→ 300 ms 内松开或鼠标在面板内：面板保留并获得焦点；否则收起
  ▼
点击“Json” → enterPlugin → 主窗口打开「JSON 编辑器」插件
  → 插件 onPluginEnter({type: "regex", payload}) → 格式化并显示
```

“有选中”和“没选中”用的是同一个触发和同一个窗口，区别只在模拟 ⌘C 之后剪贴板序号有没有变化。

## 2. 全局鼠标监听：原生插件里的 CGEventTap

原生插件：`Resources/app.asar.unpacked/node_modules/addon/darwin-arm64.node`（SHA-256 `cac18d02…737d`）。

| 结论 | 证据 |
| --- | --- |
| 用 event tap 监听全局鼠标事件 | `nm -u` 导入了 `_CGEventTapCreate`、`_CGEventTapEnable`、`_CGEventGetIntegerValueField`、`_CGEventGetLocation`、`_CFMachPortCreateRunLoopSource`、`_CFRunLoopAddSource` |
| 中键和侧键共用一个回调，长按右键另有回调 | `nm` 中的 C++ 符号：`VoicePanelTriggerEvent(Napi::CallbackInfo const&)`、`otherMouseDownEventCallback(__CGEventTapProxy*, CGEventType, __CGEvent*, void*)`、`otherMouseLongDownEventCallback(…)`、`mouseLongRightDownEventCallback(…)`、`StopVoicePanelTriggerEvent`、`SetVoicePanelProcessBlacklist`、`TriggerMouseLongDown` |
| tap 的参数 | 反汇编 `VoicePanelTriggerEvent`（0x5c68）：两处 `CGEventTapCreate`，前三个参数都是 0（`kCGHIDEventTap`、`kCGHeadInsertEventTap`、`kCGEventTapOptionDefault` 即可以修改/丢弃事件的主动 tap），事件掩码分别是 `0x98`（右键按下/松开/拖动，用于长按右键）和 `0xe000000`（第 25/26/27 位：`OtherMouseDown/Up/Dragged`，用于中键和侧键）；之后 `CFMachPortCreateRunLoopSource` → `CFRunLoopAddSource` → `CGEventTapEnable(tap, true)` |
| 回调里判断是哪个键 | 反汇编 `otherMouseDownEventCallback`（0x5368）：`cmp w1, #0x19 / #0x1a / #0x1b` 区分按下、松开、拖动；`CGEventGetIntegerValueField(event, 3)`（`kCGMouseEventButtonNumber`）与全局变量里的目标按键号比较 |
| 前台应用黑名单 | 同一函数：黑名单字符串非空时调用 `+[Utils getFrontmostApplicationName]`，再 `containsString:`，命中就原样放行事件。对应设置项 `voicePanelProcessBlacklist`（main.js 中拼成 `";" + 名单 + ";"` 传给原生层） |
| **中键点击被吞掉** | 同一函数：按下且命中时 `napi_call_threadsafe_function` 通知 JS（坐标来自 `CGEventGetLocation`），成功后 `mov x19, #0x0` 并返回 x19，即返回 NULL——前台应用收不到这次按下；松开事件同样返回 NULL；拖动事件只通知、原样返回 |
| tap 被系统停用后自动恢复 | 同一函数：`cmn w1, #0x2` 判断类型 ≥ `0xFFFFFFFE`（`kCGEventTapDisabledByTimeout` / `ByUserInput`）时 `CGEventTapEnable(tap, 1)` |
| 需要“辅助功能”权限 | main.js@330274 `isEnableVoicePanel()`：macOS 上 `isHadPrivilege()` 为假时不启用面板；`nm` 中有 `IsHadPrivilege` / `RequestPrivilege`，导入了 `AXIsProcessTrustedWithOptions` |
| 回调编号 | main.js@339911 `nativeVoicePanelTrigger(e,t,i)`：`1` 显示（`triggerShow`），`2`/`8` 移动，`3` 松开（`handleNativeMouseUp`），`7` 开始长按计时（到时调用 `triggerMouseLongDown()`），`9` 长按松开，`0` 收起 |

触发方式（`voicePanelTriggerWay`）在设置页“超级面板”分组（`dist/plugins/ffffffff/index.js`@985092）的“鼠标按键弹出”下拉框里：

| 值 | 文案 |
| --- | --- |
| 0 | 按下鼠标中键 |
| 1 | 长按鼠标中键 |
| 2 | 长按鼠标右键（macOS 默认值） |
| 4 / 5 | 按下 / 长按鼠标向后侧键 |
| 6 / 7 | 按下 / 长按鼠标向前侧键 |

main.js@330500 `isTriggerWayNumberValue(e){return[0,1,2,4,5,6,7].includes(e)}`；长按阈值 `voicePanelLongDownTriggerMS` 限制在 200～400 ms。

## 3. 读取选中内容：模拟 ⌘C，比较剪贴板序号

uTools 没有用辅助功能 API 读 `AXSelectedText`，而是模拟一次复制。

main.js@334386（`triggerShow` 中 macOS 分支）：

```js
if (this.windowCmp.isMacOs) {
  this.clipboardCmp.temporaryCancelWatch(),
  this._simulateCopyClipboardChangeCount = W().getClipboardChangeCount(),
  W().simulateKeyboardTap("c", "command"),
  n = e > o.x && e < o.x + o.width && i > o.y && i < o.y + 23
        ? {type: "window|simulatecopy", data: o, wininfo: o}
        : {type: "simulatecopy", wininfo: o};
  break
}
```

- `W()` 是原生插件；`getClipboardChangeCount` 返回 `NSPasteboard.changeCount`（插件字符串里有 `generalPasteboard`、`changeCount`），`simulateKeyboardTap` 用 `CGEventCreateKeyboardEvent` + `CGEventPost` 发按键（插件导入了这两个函数，以及换算键码用的 `TISCopyCurrentKeyboardLayoutInputSource`、`UCKeyTranslate`）
- 点在窗口标题栏（顶部 23 点）里时类型是 `window|simulatecopy`：没选中内容时改为按窗口匹配功能
- `temporaryCancelWatch()`（main.js@227241）把剪贴板记录暂停 300 ms，这次复制不会进 uTools 自己的剪贴板历史；**之后不会恢复剪贴板**，选中的内容会留在剪贴板里
- 在这之前先看 `isPreCopy()`（main.js@227097）：用户刚复制过（`autoPasteLife` 秒内）就直接用剪贴板内容，标题显示“复制的文本”

面板窗口随后 `showInactive()`（main.js@337931），显示但不激活，所以 ⌘C 仍然发给原来的应用。

判断有没有选中在渲染进程里，dist/voice/index.js@232283（`showInit`）：

```js
setTimeout(() => {
  if (window.services.voice.isSimulateCopyForClipboardUpdate()) {
    const t = window.services.getPasteCmd();
    if (t.data) return t.wininfo = e.wininfo, this.matchSearch(t)
  }
  this.matchSearch({type: "window", data: e.wininfo, wininfo: e.wininfo})
}, 50)
```

对应的主进程实现 main.js@320248：

```js
isSimulateCopyForClipboardUpdate: e => {
  e.returnValue = W().getClipboardChangeCount() > this._simulateCopyClipboardChangeCount
}
```

也就是固定等 50 ms：序号变大说明有选中内容，读剪贴板（`getPasteCmd` 区分文本 / 图片 / 文件）；没变就按当前窗口匹配。标题文案在 dist/voice/index.js@230954：

```js
getMatchLabel(e) {
  return "text" === e.type ? (e.isClipboardData ? "复制的" : "选中的") + "文本 " + e.data.length + " 个" : …
}
```

这就是截图里的“选中的文本 389 个”（`length` 是 UTF-16 码元数）。

### Windows 与 Linux 的差异（同一个 `triggerShow`）

- Windows（main.js@336481）：模拟 `Ctrl+C`；前台是 `cmd.exe` / `powershell.exe` / `WindowsTerminal.exe` 时改为 `Ctrl+Insert`，避免 Ctrl+C 中断正在运行的命令。资源管理器和桌面不模拟复制，而是用原生方法直接取选中的文件路径（`getExplorerWindowsForSelectedFilePaths`、`getDesktopSelectedFilePath`）；前台是 uTools 自己的窗口时直接调用 `webContents.copy()`
- Linux（main.js@337102）：X11 的 PRIMARY 选区本身就是“当前选中的文本”，用 `wheelMouseDownIsSelection()` 判断后直接 `clipboard.readText("selection")`，不用模拟复制。启用面板时还会执行 `xmodmap -e "pointer = 1 25 3 4 5 6 7 8 9 10"`（main.js@332651），把物理中键映射成 25 号键，避免 X11 默认的“中键粘贴”

## 4. 按选中内容匹配功能

功能由插件在 `plugin.json` 中声明。本机「JSON 编辑器」插件（`~/Library/Application Support/uTools/plugins/4a71978572eb04bd852af4f6afd8fe95.asar`，1.7.1）：

```json
{"code": "json", "explain": "Json 格式化编辑", "cmds": [
  "Json",
  {"type": "regex", "minLength": 2, "match": "/^\\s*(\\{[\\s\\S]*\\}|\\[[\\s\\S]*\\])\\s*$/", "label": "Json"},
  {"type": "files", "fileType": "file", "maxLength": 1, "match": "/\\.json$/i", "label": "Json"}
]}
```

dist/voice/index.js@210044 的匹配函数：

```js
function q(e, t, n) {
  const r = {payload: t};
  e.forEach(e => {
    e.minLength && e.minLength > t.length || e.maxLength && e.maxLength < t.length ||
      (r.regex = e.match, window.services.regexCmdTest(r) && $(n, e, 9e8, 0))
  })
}
```

即：选中的文本以 `{…}` 或 `[…]` 包住、至少 2 个字符，就出现“Json”这一项；选中单个 `.json` 文件也会出现。截图里的第二个“Json”应该来自本机装的另一个插件“全能JSON处理”，“生成qrcode”、“翻译”是匹配任意文本的功能。

## 5. JSON 编辑器插件

插件是 Monaco 编辑器（bundle 中有 `vendors-node_modules_monaco-editor_esm_vs_language_json_jsonMode_js.js`、`editor.worker.js`、`json.worker.js`）。

| 功能 | 证据（`index.js`） |
| --- | --- |
| 进入时格式化选中的文本 | @3272872 `utools.onPluginEnter(({type, payload}) => …)`：`regex` 类型调用 `setEditorFormatValue(payload)`；`files` 类型先 `readFileContent(payload[0].path)` |
| 格式化方式：文本级，不重新序列化 | @3267425 `setEditorFormatValue = e => { const t = formatText(e); editor.setValue(t); editor.getAction("editor.action.formatDocument").run() }`，所以 `"sort": 2.0` 保持原样（截图第 8 行） |
| 大数字不丢精度 | 解析器带 BigNumber（@62390 附近的 `[BigNumber Error]`） |
| 底栏按钮与快捷键 | 文案：“重新格式化「Alt + F」”“折叠全部「Alt + >」”“展开全部「Alt + Shift + >」”“去除注释”“压缩 JSON 并复制「Alt + C」”“压缩转义 JSON 并复制「Alt + \\」”“JSON 转 XML 并复制”“JSON 转 TypeScript 并复制” |
| 压缩、转义 | `compressJson`：去掉字符串外的空白；转义 = `JSON.stringify(压缩结果)` 去掉两端引号 |
| JS 过滤 | 底栏 “this” + 输入框（占位符 `JS 过滤; 示例 ".key.subkey"、"[0][1]"、".map(x=>x.val)"`），`evalJsInContext` 把输入拼在 `this` 后面执行 |
| 粘贴自动转换 | 占位文字“URL Params、XML、YAML 粘贴自动转为 JSON” |

## 6. 在本项目中的实现

| uTools | 本项目（Rust + egui） |
| --- | --- |
| 原生插件 `CGEventTapCreate(HID, HeadInsert, Default, 0xe000000)`，按键号 2，返回 NULL 吞掉点击，超时后重新启用 | `platform/macos/mouse.rs`：相同的参数和处理；没有辅助功能权限时每 3 秒检查一次，授权后自动生效 |
| Windows 版插件 | `platform/windows/mouse.rs`：`SetWindowsHookExW(WH_MOUSE_LL)`，`WM_MBUTTONDOWN/UP` 返回 1 吞掉 |
| 前台应用黑名单 | 只在本应用处于前台时放行（`middle_click.rs`）；设置里可以整体关闭 |
| 记下 changeCount → 模拟 ⌘C → 固定等 50 ms 比较 | `selection.rs`：同样的思路，改为每 10 ms 轮询、最多 300 ms，序号变化后再等 30 ms 让对方写完 |
| 选中内容留在剪贴板里，只暂停记录 300 ms | 读完把剪贴板恢复成原来的内容；整个过程在 `Shared::write_pasteboard` 里，监听线程不会把这次复制记进历史 |
| Windows 控制台用 Ctrl+Insert | `platform/windows/keyboard.rs`：`copy_keycode()` 对 cmd / PowerShell / Windows Terminal / conhost 返回 Insert |
| `showInactive()` 先显示面板再取词 | 先取词再显示：egui 窗口显示时会激活本应用，⌘C 就发不到原来的应用了。代价是没有选中内容时要等满 300 ms 才弹出 |
| `plugin.json` 的 regex / files 规则 | `tools/mod.rs`：同样只看首尾括号（另外接受转义过的 JSON 字符串），或单个 `.json` 文件；没选中时列出所有功能 |
| 标题“选中的文本 N 个” | `Selection::label()`，按字符计数 |
| Monaco + formatText 文本级格式化 | `tools/json.rs`：自己写的词法分析与格式化，保留数字原文（`2.0`、大整数）、键顺序和注释；错误给出行列号 |
| 重新格式化 / 压缩复制 / 转义复制 / 去除注释，Alt+F / Alt+C / Alt+\\ | `ui/json_editor.rs`：同样的功能和快捷键，另加“去除转义”、缩进选择（2 / 4 / Tab） |
| JS 过滤（执行 JavaScript） | 不执行代码的路径过滤：`.key.subkey`、`[0][1]`、`[-1]`、`["含空格的键"]`、`.list[].name`（相当于 `.map(x => x.name)`）、`.list.length` |
| 折叠 / 展开、转 XML / TypeScript、URL Params / XML / YAML 自动转换 | 暂未实现 |

## 7. 复现方法

```bash
# 原生插件导入与导出的符号
nm -u /Applications/uTools.app/Contents/Resources/app.asar.unpacked/node_modules/addon/darwin-arm64.node
nm /Applications/uTools.app/Contents/Resources/app.asar.unpacked/node_modules/addon/darwin-arm64.node | c++filt | grep -i -E 'mouse|voice|tap'
# 反汇编中键回调（地址来自上一条命令）
otool -tV /Applications/uTools.app/Contents/Resources/app.asar.unpacked/node_modules/addon/darwin-arm64.node \
  | awk '/^0000000000005368/{p=1} /^00000000000055b4/{p=0} p'
```

app.asar 的格式：偏移 4 处的 uint32 是文件头长度 `H`，偏移 12 处的 uint32 是 JSON 长度 `L`，`[16, 16+L)` 是描述目录树的 JSON（每个文件有 `offset`、`size`），文件内容从 `8+H` 开始。按这个格式即可切出 `dist/main.js`、`dist/voice/index.js` 和插件的 `plugin.json`、`index.js`。
