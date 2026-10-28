# AGENT.md

面向 AI 编码助手的项目约定。本文件之外，`README.md` 有完整的功能与实现说明，`docs/` 有各项功能的实现分析与踩坑记录。

## 项目概览

DevTools Hub：剪贴板历史管理器（参照 PasteNow），附带鼠标中键快捷面板（参照 uTools）、截屏 / 标注 / 贴图（参照 Snipaste）、滚动截图与文字提取（参照微信）、JSON 编辑器。

- Rust workspace（edition 2021，MSRV 1.95），GUI 用 egui / eframe
- 仅支持 macOS 与 Windows；两个平台各自实现在 `crates/devtools-hub/src/platform/{macos,windows}/`，对外暴露相同的模块和函数，编译时按目标系统二选一
- 数据存储：SQLite（rusqlite bundled）+ zstd 压缩的 blobs 目录

## 目录结构

```
crates/
├── devtools-clipboard/   平台无关：剪贴板数据模型、UTI、隐私规则、SQLite 存储
├── devtools-json/        平台无关：JSON 词法/解析/格式化/路径过滤（只依赖 serde）
└── devtools-hub/         应用本体：monitor、中键面板、截屏贴图 snip/、platform/、ui/
docs/                     实现分析与排查记录（修改对应功能前先读相关文档）
scripts/                  bundle-macos.sh、create-signing-cert.sh、generate-icons.py
.github/workflows/        ci.yml（检查与测试）、release.yml（版本发布）
```

## 常用命令

```bash
cargo check -p devtools-hub                              # 快速类型检查
cargo test --workspace                                   # 全部单元/交互测试
cargo test -p devtools-json                              # 只跑 JSON 模块（纯 Rust）
cargo check -p devtools-hub --features windows-typecheck # macOS 上对 Windows 实现做类型检查
cargo test -p devtools-hub snapshot -- --ignored         # 离屏渲染快照（需 GPU，输出 target/ui-snapshots/）
cargo run -p devtools-hub --release                      # 运行
cargo run -p devtools-hub -- --show --data-dir /tmp/dth-dev  # 开发调试（隔离数据目录）
zsh scripts/bundle-macos.sh                              # 打包 macOS .app
```

改动 UI 后如影响快照，手动重跑 snapshot 测试并人工检查输出的 PNG。

## Git 提交规范

提交信息用中文，格式：`类型: 描述`，必要时可加 scope：`类型(scope): 描述`。

| 类型 | 用途 |
| --- | --- |
| `feat` | 新功能 |
| `fix` | 修复缺陷 |
| `docs` | 文档（README、docs/ 分析与排查记录） |
| `chore` | 版本号、构建配置、.gitignore、依赖等杂项 |
| `refactor` | 重构（不改行为） |
| `perf` | 性能优化 |
| `test` | 测试补充与调整 |
| `style` | 界面样式、文案调整 |
| `build` / `ci` | 构建系统 / CI 工作流 |

示例（均来自本仓库历史提交）：

```
feat: 新增 devtools-json 模块，提供 JSON 处理能力
fix(windows): 修复 Windows 截屏时闪白和放大画面的问题
docs: 重写功能模块化架构评估，明确编译期模块化方向与渐进落地策略
chore: 版本升到 0.1.6
```

- 描述写清楚做了什么、为什么；改动较大时正文（body）补充动机与影响范围，参照仓库中带 body 的提交
- 版本号发布：修改根 `Cargo.toml` 的 `version` 并以 `chore: 版本升到 x.y.z` 提交，CI 会创建 Release 草稿
- 默认不提交：用户没有明确要求时只改代码，不执行 git 提交与推送；大功能可以先开 `feat/xxx` 分支

## 代码约定

- **注释全中文**，模块顶部 `//!` 说明职责与设计取舍，函数用 `///`；重要实现决策、平台差异、踩过的坑都写进注释或 `docs/` 排查记录
- **平台差异**用 `#[cfg(target_os = "macos")]` / `#[cfg(not(...))]` 隔离，公共逻辑放 `platform/mod.rs` 定义的接口后面；在 macOS 上改完 Windows 相关代码必须跑 `--features windows-typecheck`
- **依赖克制**：objc2 / windows 系列只启用用到的类与特性（见根 `Cargo.toml`，改了要说明）；能不引新依赖就不引
- **UI 一致性**：macOS 遵循系统习惯（如关闭按钮放左上角红绿灯位置），Windows 遵循 Windows 习惯；两个平台的交互差异记录在 README 的“两个平台的差异”表
- 剪贴板、模拟按键等系统调用会真实影响用户环境，相关测试要么用私有命名剪贴板（`--pasteboard`），要么标记 `#[ignore]` 手动跑，不许碰系统剪贴板
- egui 多视口（截屏遮罩、贴图）有坑，改之前先读 `docs/Snipaste截图贴图实现分析.md` 的“egui 多视口的几处坑”

## 完成标准

提交前：

1. `cargo check` 无警告
2. `cargo test --workspace` 通过
3. 涉及 Windows 代码时 `--features windows-typecheck` 通过
4. 涉及 UI 时快照测试已重跑并确认
