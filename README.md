# XAim

**以快速、准确、便捷为目标的开源桌面编程 Agent。** 输入任务，查看执行过程与文件差异，审阅后应用到项目。

An open-source desktop coding agent with controlled file tools, verification, and review before applying changes.

[MIT 许可证](LICENSE) · [开发与构建](docs/development.md) · [文档索引](docs/README.md) · [参与贡献](CONTRIBUTING.md)

XAim 当前处于 **0.1 早期开发阶段**，支持 Windows 和 macOS。基于 Tauri 2、Rust、React 18、TypeScript 5、Tailwind CSS 3 和 Vite 5 构建。

## 能做什么

- **编码与分析**：修复问题、补测试、重构、多文件修改，以及无需改代码的解释与审查。
- **受控执行**：在独立源码副本中读取、搜索和编辑；文件版本 hash、路径限制与唯一匹配保护编辑过程。
- **可审阅交付**：逐文件 Diff、待应用新文件预览、完整基线冲突检查，以及应用与撤销。
- **连续会话**：Markdown 回答、SQLite 历史、重开后续聊，下一轮继承待应用变更；模型恢复最近最多 24 轮、48000 字符的公开问答。
- **效率与可观测性**：流式正文、一次最多 8 文件批量读取、轻量状态持久化，以及模型、工具、文件复制与验证耗时诊断。
- **项目检查**：用户按任务授权后检测并执行项目检查，失败时有限修复；依赖不足明确记录无法验证。

接入 OpenAI 兼容服务，包括 OpenAI、DeepSeek、Qwen、NewAPI 和 Ollama 的兼容端点。具体工具与流式支持取决于模型服务；当前桌面任务使用所选单一模型。原生 Anthropic 适配器尚未实现，请勿将其视为已支持能力。

## 快速开始

环境：Node.js 22+（推荐 24 LTS）、pnpm 12.3.4、Rust stable。Windows 另需 MSVC Build Tools（C++ 桌面开发与 Windows SDK）和 WebView2；macOS 需 Xcode Command Line Tools。

```bash
git clone https://github.com/LittleCar30/XAim.git
cd XAim
pnpm install --frozen-lockfile
pnpm tauri dev
```

1. 打开项目文件夹，在设置中填写兼容 API 地址、模型与密钥；密钥保存到系统凭据库。
2. 输入具体任务，需要执行可信项目脚本时勾选“运行项目检查”。
3. 查看回答、工具记录和检查结果；编码任务审阅 Diff 后点击应用，分析任务直接给出回答。
4. 继续在当前会话追问，或从侧栏打开历史会话。

`pnpm dev` 可启动浏览器界面预览；真实模型、文件与持久化操作需要 Tauri 桌面后端。

## 执行流程与边界

```text
任务与项目约定 → 源码副本 → 模型工具循环 → 实际文件差异
                                           ↓
                              授权检查 / 明确无法验证
                                           ↓
                              用户审阅 → 基线校验 → 应用
```

项目约定支持目录内 `AGENTS.override.md`、`AGENTS.md` 或 `CLAUDE.md`，读取与搜索可使用 Tree-sitter 符号检索。具体优先级、预算、文件大小限制、流式回退和中断成果保存见 [执行内核](docs/agent-runtime.md)。

源码副本**不是操作系统沙箱**。检查脚本以当前用户权限运行；程序使用环境白名单和进程树控制，但不限制所有文件及网络访问。默认不执行项目脚本，不向模型提供任意 shell，也不自动安装依赖。项目上下文会发送到所配置服务，请按其隐私政策使用。

尚未提供工具内部中断恢复、原生 Anthropic、OS 沙箱或多代理。生产 CSP 与安装包签名尚未配置。完成状态不代表无法运行的检查已通过；已有本地回归不能证明真实模型的速度或成功率。更多内容见 [安全说明](SECURITY.md) 与 [评测记录](tests/eval/VALIDATION.md)。

## 开发与验证

```bash
pnpm test
pnpm lint
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib
cargo test --manifest-path src-tauri/Cargo.toml --locked --bin xaim-eval
pnpm test:e2e
```

普通测试使用本地模拟模型，不需要 API key。E2E 覆盖 React → Rust → 工具修改 → 验证 → Diff → 应用，不覆盖原生桌面窗口。CI 在 Windows/macOS 执行检查，PR 同样触发；Windows 额外执行 E2E。

`pnpm tauri build` 打包桌面安装程序。标签或手动触发的构建会生成 Windows x64 MSI/NSIS 与 macOS ARM64 DMG，上传为 Actions Artifacts，保留 14 天；当前不会自动创建 GitHub Release。完整构建、桌面 IPC 检查和覆盖率命令见 [开发与构建](docs/development.md)。

## 项目结构

| 路径 | 用途 |
| --- | --- |
| `src/` | React 界面、状态与 IPC |
| `src-tauri/src/` | Rust Agent、模型、文件工具、验证与数据存储 |
| `scripts/` | 集成测试、桌面检查与图标工具 |
| `tests/eval/` | 30 项评测任务、参考解和独立验收 |
| `docs/` | 实现说明、开发指南与设计归档 |
| `.github/` | CI、安装包构建与协作模板 |

问题与建议请提交 [Issue](https://github.com/LittleCar30/XAim/issues)，代码贡献见 [CONTRIBUTING.md](CONTRIBUTING.md)。项目采用 [MIT](LICENSE) 许可证；第三方依赖遵循各自许可证。
