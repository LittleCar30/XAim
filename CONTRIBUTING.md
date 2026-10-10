# 参与 XAim

欢迎通过 Issue 描述问题、讨论方案，通过 Pull Request 提交改进。提交前请先阅读 [README](README.md) 和 [执行内核](docs/agent-runtime.md)。安全问题请按 [SECURITY.md](SECURITY.md) 私下报告。

## 本地开发

使用 Node.js 24 LTS、package.json 指定的 pnpm、Rust stable。Windows 需要 MSVC Build Tools 和 WebView2，macOS 需要 Xcode Command Line Tools。完整命令和测试边界见 [开发与构建](docs/development.md)。

```bash
git clone https://github.com/LittleCar30/XAim.git
cd XAim
pnpm install --frozen-lockfile
pnpm tauri dev
```

## 提交改动

1. 从最新 main 创建分支，围绕一个问题提交改动。描述触发条件、预期行为和实际行为。
2. 有行为变化时更新相关文档，必要时添加能证明问题和修复的测试。不要只测试实现细节。
3. 前端运行 `pnpm test`、`pnpm lint`、`pnpm build`；Rust 运行下列检查。跨前后端流程的修改还需运行 `pnpm test:e2e`。
4. 提交 PR，写明改动原因、验证结果及未验证的环境。不要提交密钥、个人配置、数据库、构建产物或原始模型报告。

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib
cargo test --manifest-path src-tauri/Cargo.toml --locked --bin xaim-eval
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --lib --bin xaim-eval
```

保持 TypeScript strict 和 rustfmt 格式；新增依赖必须有明确用途，并提交对应锁文件。文件工具需保留路径与版本校验，检查结果不能把无法运行记为通过，应用变更不能覆盖用户的新修改。

贡献按项目的 [MIT 许可证](LICENSE) 发布。第三方依赖仍遵循各自许可证。线上模型评测会产生费用，请自行配置并明确授权；普通测试使用本地模拟服务。
