# 开发与构建

## 技术栈

Tauri 2 · Rust（edition 2021，stable，由 `rust-toolchain.toml` 选择；未固定具体编译器版本）· React 18 · TypeScript 5（strict）· Tailwind CSS 3 · Vite 5

## 环境要求

- Node.js 22+（推荐 24 LTS）、pnpm 12.3.4（见 package.json）
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
| `node node_modules/@tauri-apps/cli/tauri.js build -- --locked` | 使用冻结 Cargo 锁文件打包桌面安装程序 |
| `node node_modules/@tauri-apps/cli/tauri.js build --target x86_64-pc-windows-msvc --bundles msi nsis --ci -- --locked` | 打包 Windows x64 MSI 和 NSIS 安装程序 |
| `pnpm lint` | ESLint 检查 |
| `pnpm typecheck` | `tsc --noEmit` 严格类型检查 |
| `pnpm test` | Vitest 单元测试 |
| `pnpm test:e2e` | React 工作区与真实 Rust 后端的完整流程集成测试 |

## Windows 安装包

在 Windows 上执行上表的 Windows x64 打包命令。直接调用 Tauri CLI 可保留传给 Cargo 的 `-- --locked`，避免 pnpm 12 转发参数时丢失分隔符。
安装包输出到 `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/`：

- `msi/XAim_<version>_x64_en-US.msi`
- `nsis/XAim_<version>_x64-setup.exe`

Rust release 使用 thin LTO、单个 codegen unit，并移除调试信息。
NSIS 默认按当前用户安装；MSI 默认按整机安装。
安装需要 WebView2 运行时，安装器会在未安装时下载运行时。
代码签名为可选项，当前产物未签名。

## GitHub Actions

推送代码或提交 PR 后，`ci.yml` 在 Windows 和 macOS 上运行前端测试、Lint、包含类型检查的构建、Rust 格式检查、库/评测运行器测试、Clippy 和审查评分器测试；Windows 额外运行 React → Rust E2E。依赖安装使用冻结的 pnpm/Cargo 锁文件，
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

Windows 发布版另需检查真实 IPC，避免测试进程的线程栈掩盖桌面问题。
启动待测 EXE 时临时设置 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=19371`，
然后运行 `node scripts/check-desktop-ipc.mjs http://127.0.0.1:19371`。
应用需有可加载的模型配置；脚本只向不存在的项目发送三次任务请求并检查后续 IPC 响应，
不调用模型、不创建任务、不改配置。退出待测实例后恢复该环境变量；正常启动不启用调试端口。
该检查补充桌面任务入口覆盖，不代替完整工作流测试。修复背景见 [桌面栈溢出分析](desktop-stack-overflow.md)。

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

应用数据、备份恢复、任务工作区和项目设置见 [数据存储说明](data-storage.md)。

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
