# Agent 恢复性回归与经验

## 验证方法

参照 [OpenAI Codex prompting guide](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide) 的持续执行、实现后验证要求，以及 [Claude Code best practices](https://code.claude.com/docs/en/best-practices) 的可执行验收与反馈循环。这里只记录 XAim 已实现和已验证的行为，不据此宣称与其他产品具有相同能力或成功率。

每次改进遵循：读取最近任务和失败证据，提取最小复现，添加会失败的回归，修复根因，再运行相关测试及完整检查。经验落到测试和运行时规则，不能只留在聊天记录或提示词中。线上服务故障与本地实现缺陷分别核实。

## 2026-10-21 失败分类

已只读核对本机最近任务：63/64 因 Windows Python 启动错误 193 受阻，65 完成；60/61 因快照上限失败，59 因 SSE 读取中断失败，58 因未支持的开发命令受阻。不将私人对话内容、项目源码、模型密钥或数据库提交到仓库。

| 失败类型 | 恢复原则 | 防复发位置 |
| --- | --- | --- |
| 不支持的工具、缺失的程序或项目脚本 | 返回可操作的工具错误，让模型改用现有能力；授权拒绝仍停止 | `tools/operations.rs`、`agent/tool_loop.rs` 的 unavailable / unsupported 测试 |
| Windows 错误 193/216 | 解释器解析优先选可执行程序；格式或架构不兼容按环境缺失返回，保留系统错误 | `verify/runner.rs` 的 Windows 程序解析测试；`invalid_windows_executable_is_recoverable` |
| SSE 读取或连接中断 | 不执行残缺工具，保留中断正文，有限退避重试；成功响应后恢复额度 | `model_retry_allowance_resets_after_a_successful_response`、评测运行器 `broken_stream_saves_completed_candidate_and_text_without_executing_new_tools` |
| 协议错误被当作连接故障 | 仅重试明确的传输中断，避免重复请求无效角色、索引或 JSON 响应 | `transient_stream_errors_are_retryable_but_protocol_errors_are_not` |
| 连续服务故障、用户取消 | 重试次数有上限；取消应打断退避等待，不再发送请求 | `consecutive_model_failures_stop_after_bounded_retries`、`cancellation_interrupts_model_retry_backoff` |
| 收尾期间断流被误计为有效收尾 | 只有成功响应才消耗两次收尾机会；失败仍受独立重试次数、任务时限和总执行轮数限制 | `finishing_stream_failures_preserve_the_completion_allowance`、`finishing_stream_failures_still_stop_after_bounded_retries` |
| “不要安装依赖”被关键词误判为必做操作 | 识别操作短语前明确的中英文否定约束；后续肯定要求仍需实际证据，工具授权不变 | `explicitly_negated_operations_are_constraints_instead_of_requirements`、`negative_operation_constraint_does_not_block_file_work` |
| 网关重复或遗漏 tool call ID | 在执行前规范化 ID，并同步请求历史中的工具配对 | `duplicate_call_ids_are_rewritten_instead_of_failing_the_round`、`reused_tool_call_id_across_rounds_does_not_fail_the_task` |
| SSE 中遗漏 tool call ID 被解析器过早拒绝 | 在完整响应校验时保留函数名和参数校验，允许运行时生成安全且唯一的 ID | `complete_stream_without_call_id_reaches_runtime_normalization` |
| 大量细碎 SSE 分片触发接收上限 | 按输出预算放宽软接收上限，同时保留硬上限；区分网络中断、协议错误和超限 | `sse_framing_overhead_does_not_truncate_a_valid_reasoning_response`、`oversized_event_without_delimiter_is_bounded` |
| 原项目有大文件 | 建立快照时跳过并向用户和模型说明缺口，保留原文件 | `oversized_files_are_skipped_with_a_report_instead_of_failing`、`oversized_project_file_does_not_fail_the_task_baseline` |
| 候选文件变大后被漏读 | 差异、变更路径和指纹必须完整；拒绝把漏读结果当成删除或无变更 | `oversized_candidate_is_never_reported_as_a_deleted_file`、`oversized_new_candidate_cannot_disappear_from_review` |
| 克隆目录非空 | 返回工具错误，保留原目录，允许模型继续；不清空或覆盖目标 | `clone_destination_conflict_returns_to_the_model_without_overwriting` |
| 编辑后又请求克隆 | 拒绝替换基线，但保留候选编辑并返回模型处理 | `clone_after_edits_is_recoverable_and_preserves_the_candidate` |
| 创建已经存在的文件 | 提示读取后编辑或追加，不覆盖文件 | `create_file_reports_an_actionable_message_when_the_file_exists` |
| 真实模型测试收到 401 | 检查测试是否加载了系统凭据；不能直接把数据库中的凭据引用当成 API Key | `unresolved_credential_references_never_reach_a_provider`；显式 live 测试通过原生凭据库解析配置 |

本轮先复现了候选大文件漏读和重试额度未恢复，再修复并扩展回归。目录非空的预检本身已经返回工具错误，本轮补充了工具循环级验证；另修复了编辑后克隆直接终止整轮的路径。

真实模型重放首次返回 401 后，确认旧 live 测试直接反序列化配置，未解析凭据引用；这不是证明用户密钥失效的证据。已修正测试，并让 provider 工厂在本地拒绝未解析的引用。常规单测继续使用内存凭据，不读取用户系统凭据；只有显式启用的 ignored live 测试读取原生凭据库。

修正凭据加载后的原始任务重放在约 266 秒后失败。独立日志确认已生成并保存一份 HTML 补丁，但后续两次 SSE 读取均失败，失败请求被误计入两次收尾机会，导致重试额度尚未耗尽就停止。该次重放不计为模型任务完成或 UI 验收通过。随后先用两项回归复现，再修正收尾计数；持续失败时仍按三次重试上限终止，保留候选补丁且不应用到原项目。

公开 HTML 任务首次运行在模型请求前被误判为缺少命令授权，原因是 `Do not install dependencies` 命中了肯定操作关键词。这是本地任务分类缺陷，不是模型或 API 故障。补充直接否定、否定后仍有肯定要求，以及文件任务完整流程回归后修复；不据此宣称支持任意自然语言否定范围的完整语义分析。

原始私人任务的重放在提高 SSE 接收预算后仍曾多次断流。细碎 reasoning 分片使事件封装开销达到约 262 KB，暴露了旧预算按输出 token 的 64 倍估算过紧；预算现提高到 512 倍但仍受硬上限约束。另一个完整但缺少工具调用 ID 的 SSE 响应此前在解析层被拒绝，尽管 Agent 已有安全 ID 规范化逻辑；现在该响应能到达运行时规范化，缺少名称或无效参数仍会拒绝。两项修复均已添加针对性回归。原始私人任务尚未在这些修复后完成重放，因此不记录为已解决的端到端任务。

随后再次以独立日志重放原始任务：前两轮模型响应完成并调用工具，第三轮在 34.8 秒后以 `length` 结束。运行时丢弃这轮未完成的工具调用；此前的一次有界提高输出预算重试已经用尽，最后保存候选和阶段进展，但未应用变更。全程约 224 秒，故原始任务仍未通过端到端验收。此失败与 SSE 接收上限、缺失调用 ID 属于不同问题；重复加大线上重试会带来额外耗时和 token 成本，目前保留上下文与总预算检查下的单次截断恢复策略。

## 检查入口

```bash
pnpm test
pnpm lint
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib
cargo test --manifest-path src-tauri/Cargo.toml --locked --bin xaim-eval
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --lib --bin xaim-eval
node --test tests/eval/review-validation.test.mjs
pnpm test:e2e
```

本地回归使用模拟模型和临时项目，不消耗线上模型额度。真实模型测试单独运行并保留独立日志，测试中的单任务时限为 300 秒，日志数据库必须与桌面数据库分开。不能把本地回归数量等同于线上任务成功率；也不能仅凭 HTML/SVG 结构检查宣称视觉效果已验收。

显式 live 测试可用 `XAIM_LIVE_PROMPT` 提供公开、可复现的任务替代私人对话重放；未设置时仍只读加载最新任务。此变量不会修改桌面数据库。仅输出协议元数据，凭据仍由系统凭据库加载。

## 本轮验证记录

2026-10-21，Windows 本机：

| 检查 | 结果 |
| --- | --- |
| Rust 库测试 | 695 通过，3 个显式外部/集成测试默认忽略；涵盖本轮恢复性回归 |
| 前端单测 | 154 通过 |
| 评测运行器 | 14 通过，覆盖中断后保留候选与正文、禁止执行未完成工具 |
| 审查验证器 | 2 通过 |
| React → Rust 完整流程 | 1 通过，覆盖临时项目、索引、任务、Diff、应用及结果核验 |
| 真实 GitHub 克隆 | 显式运行通过，确认 Git 元数据和仓库文件交付 |
| 真实模型公开 HTML 流程 | 约 16 秒、4 次模型请求，通过生成、读取、提交审阅、应用和回滚；使用独立日志 |
| 原始私人任务重放（最终修复后） | 约 224 秒；两轮工具调用后再次达到模型输出长度上限，候选保留、未应用；未通过端到端验收 |
| 公开 HTML 浏览器交互 | 页面显示蓝色 SVG 圆形；实际点击按钮后标题改变。只代表此最小样例，不代表原始任务视觉验收 |
| lint / 生产构建 / Rust 格式 / 桌面入口编译 / Clippy | 通过；保留既有 watcher `while_let_loop` 和构建体积提示 |

原生桌面窗口与安装包本轮未验收。真实模型重放的结果单独记载，不计入上述模拟模型回归数量。
