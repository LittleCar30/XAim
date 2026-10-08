# 评测集验收

2026-10-07 完成全部 30 个任务的实际运行。每个目录均包含 `repo/`、`task.md`、`verify.sh`，任务间不引用彼此源码，验证器随任务目录提供。

| 类别 | 数量 | 初始代码通过 | 参考解通过 |
| --- | --- | --- | --- |
| Bug 修复 | 10 | 0 | 10 |
| 补测试 | 5 | 0 | 5 |
| 重构 | 5 | 0 | 5 |
| 跨文件 | 5 | 0 | 5 |
| 依赖升级 | 3 | 0 | 3 |
| 代码审查 | 2 | 0 | 2 |
| 合计 | 30 | 0/30（0%） | 30/30（100%） |

参考解作为候选目录运行 `candidate` 模式也通过 30/30，用于验证候选目录协议、文件范围限制和报告生成。整目录复制的单任务通过 Bash 验证入口。

执行环境：Node.js 24.13.0、Python 3.11.9、Rust 1.98.1、Go 1.24.0、Git Bash。Go 可执行文件通过 `EVAL_GO` 指定；Python wheel 0.45.1 安装在 `.tools/python-build/`，通过 `PYTHONPATH` 指定。环境工具不属于任务源码。

详细日志：`reports/baseline.json`、`reports/reference.json`、`reports/reference-candidate.json`。报告记录每个任务的结果、日志或失败原因、耗时与分类成功率。

以上为评测集和参考解的运行结果，尚未调用 XAim 模型完成这些任务，因此没有 XAim 的实际准确率。使用 XAim 输出的 30 个候选目录运行 `node tests/eval/run.mjs candidate <candidate-root>` 即可计算其成功率，缺失候选计失败。

## Rust 自动运行器验收（2026-10-08）

运行器位于 `runner.rs`，通过 `src-tauri` 的 `xaim-eval` 二进制复用 XAim 编排器。默认 agent 模式执行真实任务，reference/baseline 模式用于验收任务和运行器。

| 检查 | 结果 |
| --- | --- |
| `cargo test --manifest-path src-tauri/Cargo.toml --locked --bin xaim-eval` | 8/8 通过 |
| `npm run eval -- --mode reference` | 30/30（100%），平均 3325 ms，退出 0 |
| `xaim-eval --mode baseline` | 0/30（0%），平均 2288 ms，退出 0 |
| `npm run eval -- --help` | 参数帮助正常，退出 0 |
| Rustfmt 与 `node --check tests/eval/prepare.mjs` | 通过 |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --locked --bin xaim-eval --no-deps` | 通过，仅现有 `src/index/watcher.rs` 循环写法警告 |

8 项测试覆盖任务排序、缺失任务文件、临时目录隔离和清理、空目录与报告写入错误、带空格的路径、命令超时和超时前日志、依赖准备失败、缺失模型配置、模型失败与验证结果独立记录。脚本模型还通过真实编排器生成并应用补丁，验证 4 次模型调用的输入 44 / 输出 28 / 合计 72 token。

最终批量运行使用上述 Go、Python 环境与 Windows Git Bash。全部 30 个目录包含必需的 `repo/`、`task.md`、`verify.sh`。两份 JSON 均记录 30 个任务且无报告级错误；基线每项均实际执行验证并返回非零，参考解每项执行和验证均成功。测试包含 Windows 进程 Job 的超时清理；Unix 进程组处理尚未在本机执行。

可读报告：[参考解](reports/rust/reference.md)、[初始代码](reports/rust/baseline.md)。原始数据：[参考解 JSON](reports/rust/reference.json)、[初始代码 JSON](reports/rust/baseline.json)。reference/baseline 不调用模型，token 为 0。

尚未提供在线模型配置，因此没有测量真实模型的 XAim 成功率。使用 `npm run eval -- --config /path/to/config.json` 自动跑全部任务，生成 `reports/rust/agent.json` 和 `reports/rust/agent.md`；配置示例见 `config.example.json`。
