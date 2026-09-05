# DevTools Hub

DevTools Hub 是一个原生桌面开发者效率工具原型，使用 Rust、Slint、SQLite 和 Tokio 构建。

## 当前阶段能力

- 基于 Slint 的主搜索窗口，默认中文简体界面，并预留英文切换。
- 全局快捷键唤起主窗口，默认 `Alt + Space`。
- SQLite 初始化、迁移、设置存储和执行历史记录。
- 剪贴板轮询、持久化、全文搜索，并尽量记录复制来源窗口标题。
- 内置 JSON 格式化、压缩、校验工具。
- 内置 Base64 编码、解码工具。
- 剪贴板历史作为工具入口打开，独立窗口展示和搜索记录。
- 工具按需打开独立窗口，支持原生最大化、拖拽缩放和置顶。
- 托盘菜单支持显示主窗口和退出，单击托盘图标可显示主窗口。
- 鼠标中键快捷动作窗口：选中文本时可快速选择适合的工具，例如 JSON 格式化。
- 插件命令模型预留多语言字段，后续插件可以声明本地化标题和副标题。
- Phase 2 WASM 插件：扫描工作区和用户数据目录下的 `plugins/<id>/plugin.toml`，注册插件命令到搜索和工具列表，并通过受限 WASM 宿主执行。
- 内置 JWT 离线解析、URL 编码/解码、时间戳转换、UUID 批量生成/规范化和基础 SQL 格式化。
- 工具按本次启动内的最近使用和累计使用次数排序；输入变更自动计算输出，并提供可配置的每工具历史记录。

## 运行方式

```powershell
cargo run -p devtools-app
```

macOS 打包应用（dock 图标来自应用包）：

```sh
zsh packaging/macos/build-app.sh
open "target/release/DevTools Hub.app"
```

## 默认快捷键

- Windows/Linux：`Alt + Space`
- macOS：`Option + Space`

应用数据会存放在系统应用数据目录下的 `DevToolsHub` 目录中。

WASM 插件 ABI、资源限制和权限约束见 [docs/wasm-plugin-abi.md](docs/wasm-plugin-abi.md)。插件默认没有运行时权限；声明高权限能力的插件会被拒绝执行，直到后续设置页提供显式授权。

## 注意事项

中键快捷动作会尽力读取当前选中文本：程序会短暂发送系统复制快捷键，读取剪贴板内容，然后恢复原剪贴板文本。macOS/Linux 上可能需要辅助功能、输入监听等系统权限，后续还需要做平台适配增强。
