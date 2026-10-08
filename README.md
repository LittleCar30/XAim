# XAim

桌面编程 Agent，当前为内部测试版本。OpenAI 兼容模型使用受控工具循环，最终变更经用户审阅后应用。

## 执行内核

任务在临时源码副本中读取、搜索、创建和编辑文件，包含当前工作区的未提交与未跟踪源码。
已有文件编辑需要读取版本的 hash、非空且唯一的匹配文本；文件路径不得越界或指向链接、Git 元数据和敏感文件。
程序根据任务前后内容生成逐文件 Diff，不要求模型生成补丁或默认生成三个候选。

模型提出完成后，程序执行最终检查。检查失败会回到模型修复，最多三轮；检查无法运行时明确记录为无法验证。
输入框的“运行项目检查”仅授权本次任务。默认不执行仓库脚本；授权后只运行自动检测的检查命令，
不向模型开放任意命令工具。日志记录命令、工作目录、结果、用量及每轮响应的结束原因和耗时；思考内容仅按协议回传，不展示或保存到历史。
项目脚本仍以当前用户权限运行，隔离副本不是操作系统沙箱，也不限制其网络或其他文件访问。

任务默认最多 32 次模型响应、96 次工具调用、500000 总 token，工具循环最长 30 分钟。
输出预算按已知模型能力设置，未知模型使用保守值；用户显式额度是硬上限。
上下文预检复用上一轮服务端报告的提示 token 数，并对新增消息做保守估算；缺失用量时按预留额度计入任务预算。
截断响应在执行工具前整轮丢弃，预算允许时重试一次，不重复执行已完成的编辑。
长文件按 offset 分段读取，工具输出有长度限制；上下文耗尽时停止并保留历史，不自动应用变更。

确认仅应用任务产生的文件变更，不自动初始化仓库、Git commit、reset 或清理用户文件。
应用与撤销均比较文件完整版本；用户随后修改过相关文件时拒绝覆盖。新任务的基线和执行事件持久化到 SQLite。
旧任务没有文件基线时需要重新运行，不能直接应用或撤销。

当前限制：源码副本不复制 `node_modules`、`target`、虚拟环境、构建输出、敏感文件或链接；
依赖未安装的项目可能无法验证，不会自动安装依赖。快照最多 50000 文件、单文件 32 MB、总计 256 MB；
文件工具支持不超过 1 MB 的 UTF-8 文本。尚未提供自动上下文压缩、原生 Anthropic 接入、OS 沙箱或多代理。

## 技术栈

Tauri 2 · Rust（edition 2021，stable，由 `rust-toolchain.toml` 锁定）· React 18 · TypeScript 5（strict）· Tailwind CSS 3 · Vite 5

## 环境要求

- Node.js ≥ 18、pnpm
- Rust stable（通过 [rustup](https://rustup.rs) 安装）
- Windows：MSVC Build Tools（"使用 C++ 的桌面开发"工作负载，含 Windows SDK）；WebView2 运行时（Win10/11 自带）
- macOS：Xcode Command Line Tools

## 常用命令

| 命令 | 说明 |
| --- | --- |
| `pnpm install` | 安装依赖 |
| `pnpm tauri dev` | 启动桌面开发窗口（自动拉起 Vite） |
| `pnpm dev` | 仅前端，浏览器访问 http://127.0.0.1:5173 |
| `pnpm build` | 类型检查 + 前端产物构建（输出 `dist/`） |
| `pnpm tauri build` | 打包桌面安装程序 |
| `pnpm tauri build --target x86_64-pc-windows-msvc` | 打包 Windows x64 MSI 和 NSIS 安装程序 |
| `pnpm lint` | ESLint 检查 |
| `pnpm typecheck` | `tsc --noEmit` 严格类型检查 |
| `pnpm test` | Vitest 单元测试 |
| `pnpm test:e2e` | React 工作区与真实 Rust 后端的完整流程集成测试 |

## Windows 安装包

在 Windows 上执行 `pnpm tauri build --target x86_64-pc-windows-msvc`。
安装包输出到 `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/`：

- `msi/XAim_0.1.0_x64_en-US.msi`
- `nsis/XAim_0.1.0_x64-setup.exe`

Rust release 使用 thin LTO、单个 codegen unit，并移除调试信息。
NSIS 默认按当前用户安装；MSI 默认按整机安装。
安装需要 WebView2 运行时，安装器会在未安装时下载运行时。
代码签名为可选项，当前产物未签名。

## GitHub Actions

推送代码后，`ci.yml` 在 Windows 和 macOS 上运行 `pnpm test`、`pnpm lint`、
`pnpm typecheck` 和 Rust 库测试。依赖安装使用冻结的 pnpm/Cargo 锁文件，
并缓存 pnpm store 和 Rust 编译产物。

推送 tag（例如 `v0.1.0`）后，`release.yml` 自动构建 Windows x64 MSI/NSIS
和 macOS ARM64 DMG。安装包上传到该次 Actions 运行的 Artifacts：
`xaim-windows-x64`、`xaim-macos-arm64`，保留 14 天。
当前流程无需签名密钥，上传的安装包未签名。
两个工作流也支持在 GitHub Actions 页通过 Run workflow 手动触发。
Rust 测试和各平台 release 构建分别使用缓存键。
Windows 安装包上传前会检查体积，达到或超过 20 MB 时构建失败。

## 端到端集成测试

```bash
pnpm test:e2e
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib commands::e2e::complete_project_to_index_task_diff_apply_and_verify_flow
```

`pnpm test:e2e` 自动编译 Rust 测试进程，并在 Vitest/jsdom 中操作完整 React 工作区：
打开临时项目 → 真实索引 → 输入任务 → 原生工具读取与编辑 → 隔离副本中的最终验证 → 显示 diff → 点击应用 → 执行真实 `cargo test`。
测试项目的加法函数最初误用减法，修复前测试必须失败，应用补丁后必须通过；
同时检查数据库补丁 ID、已应用标记、完成事件、文件内容和 Git 工作区。

模型请求只发送到 `mockito` 的本地 HTTP 服务，不需要 API key 或真实 API。
Rust 测试使用 `tempfile`，退出后自动清理项目。需要 Rust stable（含 clippy）、
Node.js 和已安装的前端依赖。测试只替换 Tauri 的原生 IPC 传输及 Monaco 渲染器，
IPC 包装、组件、stores、Rust 命令业务、索引、模型 HTTP 协议、补丁及验证执行器均真实运行。
应用后的验证由测试专用诊断命令调用真实验证器，不代表应用增加了验证按钮。
此测试不启动桌面窗口，因此不覆盖 WebView2、原生窗口或 Tauri 运行时的 IPC 权限。

## Rust 单元测试与覆盖率

在 `src-tauri/` 目录运行：

```bash
cargo test --locked
cargo install cargo-tarpaulin --version 0.37.5 --locked
cargo tarpaulin --config tarpaulin.toml
```

测试位于各 Rust 模块的 `#[cfg(test)]` 中，文件系统测试使用 `tempfile`，
HTTP 测试使用 `mockito` 本地服务，无需 API key 或外部模型服务。
覆盖 store CRUD、index 扫描/hash/语言/符号、search 检索/上下文、model mock provider、
全部工具、verify 解析器及 agent 状态机与任务流程。

`tarpaulin.toml` 使用跨平台 LLVM 引擎统计整个库的行覆盖率（不计测试代码），
低于 60.1% 时命令失败。报告写入 `src-tauri/coverage/`：
`tarpaulin-report.html`、`cobertura.xml`、`tarpaulin-report.json`、`lcov.info`。
覆盖率构建单独使用 `target/coverage/`；首次运行需重新编译依赖。

## 目录结构

```
src/            React 前端（路径别名 @/* 指向这里）
  lib/          纯逻辑与测试
src-tauri/      Rust 后端、Tauri 配置与窗口定义
  capabilities/ Tauri 2 权限声明
scripts/        开发辅助脚本（占位图标生成等）
```

## 占位图标

`app-icon.png` 由 `node scripts/make-icon.mjs` 程序化生成（无第三方图片依赖）。
替换正式图标时，把新的 1024x1024 PNG 放到根目录后执行：

```bash
pnpm tauri icon app-icon.png
```

## 备注

- `tauri.conf.json` 中 `app.security.csp` 暂设为 `null` 以保证开发期 HMR 正常；进入正式开发前应配置内容安全策略。
- Tauri 自定义命令示例：`src-tauri/src/lib.rs` 中的 `greet`，前端通过 `invoke('greet', { name })` 调用。
