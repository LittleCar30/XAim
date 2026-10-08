//! 验证-修复循环：验证失败 → 收集诊断 → 模型补丁 → 应用 → 重验。
//!
//! 流程：
//! 1. 运行项目全部验证命令（[`crate::verify::detect`] + [`crate::verify::runner`]）；
//! 2. 全部通过 → `Success`（首次即通过也走此出口，不调用模型）；
//! 3. 失败 → 输出解析为诊断错误（[`crate::verify::parser`]）；
//! 4. 渲染修复 prompt；
//! 5. 调用模型产出补丁；
//! 6. 应用补丁（应用失败只作废本轮，不终止循环）；
//! 7. 重新验证；通过 → `Success`，否则下一轮；
//! 8. 轮次超过 `max_rounds` → 回滚 + `Exceeded`；
//! 9. 总超时 → 回滚 + `Timeout`；任务被取消 → 回滚 + `Cancelled`。
//!
//! 可见性：每轮开始发 `XaimEvent::Repair`，每条验证结果与每次模型调用
//! 分别发 `Verify`/`Token` 事件；每轮与每次退出都落 tracing 日志。任务终态
//! （`Done` 事件与库内状态流转）由调用方负责。
//! 回滚：除 `Success` 外的所有出口（含错误传播）都会把工作区恢复到循环
//! 启动时的快照（[`crate::git::ops::create_rollback_point`]，非 git 项目自动 init）。
//!
//! 事件出口经 [`EventSink`] 抽象注入：生产环境由 [`repair_loop`] 转发到
//! Tauri 事件通道，单元测试用内存收集器，避免在测试中构造 Tauri 运行时。

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::errors::Result;
use crate::events::{self, XaimEvent};
use crate::model::provider::{CompletionRequest, Message, ModelProvider};
use crate::prompts;
use crate::store;
use crate::tools::exec::truncate_middle;
use crate::tools::patch;
use crate::tools::r#trait::ToolContext;
use crate::types::TaskStatus;
use crate::verify::detect::detect_verify_config;
use crate::verify::parser::{self, DiagError};
use crate::verify::runner::{self, VerifyKind, VerifyResult, DEFAULT_TIMEOUT_SEC};

/// 进入提示词的诊断条数上限，防止海量错误撑爆上下文。
const MAX_DIAGNOSTICS: usize = 30;
/// 单个失败项的原始输出进入提示词的字符上限（保留头尾）。
const MAX_RAW_OUTPUT_CHARS: usize = 4_000;
/// 单个相关代码文件进入提示词的字符上限（保留头尾）。
const MAX_CODE_SNIPPET_CHARS: usize = 2_000;
/// 进入提示词的相关代码文件数上限（按诊断出现顺序取前 N 个）。
const MAX_CODE_FILES: usize = 4;

/// 修复循环的终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairResult {
    /// 验证全部通过（含首轮即通过），工作区改动保留。
    Success,
    /// 用尽 `max_rounds` 仍未通过，工作区已回滚。
    Exceeded,
    /// 总超时，工作区已回滚。
    Timeout,
    /// 任务被用户取消，工作区已回滚。
    Cancelled,
}

impl RepairResult {
    /// 终态标识，供调用方映射到 `Done` 事件与任务状态。
    pub fn as_str(self) -> &'static str {
        match self {
            RepairResult::Success => "success",
            RepairResult::Exceeded => "exceeded",
            RepairResult::Timeout => "timeout",
            RepairResult::Cancelled => "cancelled",
        }
    }
}

/// 修复循环配置。
#[derive(Debug, Clone)]
pub struct RepairConfig {
    /// 最多修复轮数（每轮 = 一次模型调用 + 一次重验）。
    pub max_rounds: u32,
    /// 单条验证命令的超时秒数。
    pub verify_timeout_sec: u64,
    /// 整个修复循环的总超时秒数。
    pub total_timeout_sec: u64,
    /// 修复用模型名（对应路由 Strong 档）。
    pub model: String,
}

impl Default for RepairConfig {
    fn default() -> Self {
        Self {
            max_rounds: 3,
            verify_timeout_sec: DEFAULT_TIMEOUT_SEC,
            total_timeout_sec: 1800,
            // 与 model::router 默认 Strong 档一致。
            model: "deepseek-reasoner".to_string(),
        }
    }
}

/// 一条验证失败的记录。
struct VerifyFailure {
    kind: VerifyKind,
    command: String,
    result: VerifyResult,
}

/// 事件出口抽象：循环产生的 [`XaimEvent`] 由此交给外界。
/// `Send + Sync` 上界保证持有它的 async fn 未来仍是 `Send`；
/// 公开供调用方注入自定义通道（默认实现走 [`TauriSink`]）。
pub trait EventSink: Send + Sync {
    fn emit(&self, event: XaimEvent) -> Result<()>;
}

/// 生产事件出口：转发到 Tauri 全局事件通道。
struct TauriSink<'a>(&'a AppHandle);

impl EventSink for TauriSink<'_> {
    fn emit(&self, event: XaimEvent) -> Result<()> {
        events::emit(self.0, event)
    }
}

/// 驱动验证-修复循环至终态。
///
/// 返回 `Ok(RepairResult)` 表示循环正常收敛或终止（终态见 [`RepairResult`]）；
/// `Err` 表示循环本身出现故障（模型调用链、存储、补丁引擎等），此时工作区
/// 同样已回滚。
pub async fn repair_loop(
    task_id: i64,
    ctx: &ToolContext,
    provider: &dyn ModelProvider,
    config: &RepairConfig,
    app: &AppHandle,
) -> Result<RepairResult> {
    let sink = TauriSink(app);
    repair_loop_with_sink(task_id, ctx, provider, config, &sink).await
}

/// [`repair_loop`] 的可注入变体：事件出口显式传入（生产经 TauriSink 转发
/// 事件通道，测试用内存收集器），其余语义完全一致。
pub async fn repair_loop_with_sink(
    task_id: i64,
    ctx: &ToolContext,
    provider: &dyn ModelProvider,
    config: &RepairConfig,
    sink: &dyn EventSink,
) -> Result<RepairResult> {
    let root = ctx.project_root.clone();
    let deadline = Instant::now() + Duration::from_secs(config.total_timeout_sec);

    // 任务前快照：非 git 项目会被自动 init，保证任何出口都可回滚。
    let rollback_point = crate::git::ops::create_rollback_point(&root, task_id)?;
    tracing::info!(
        task_id,
        rollback_point = %rollback_point,
        max_rounds = config.max_rounds,
        total_timeout_sec = config.total_timeout_sec,
        "修复循环启动"
    );

    let outcome = drive_rounds(task_id, ctx, provider, config, sink, deadline).await;
    match &outcome {
        Ok(RepairResult::Success) => {
            tracing::info!(task_id, "验证通过，保留工作区改动");
        }
        Ok(other) => {
            tracing::warn!(
                task_id,
                result = other.as_str(),
                "修复未收敛，回滚到任务前状态"
            );
            if let Err(e) = crate::git::ops::rollback(&root, &rollback_point) {
                tracing::error!(task_id, error = %e, "回滚失败");
            }
        }
        Err(e) => {
            tracing::error!(task_id, error = %e, "修复循环异常，回滚到任务前状态");
            if let Err(e) = crate::git::ops::rollback(&root, &rollback_point) {
                tracing::error!(task_id, error = %e, "回滚失败");
            }
        }
    }
    outcome
}

/// 轮次驱动：验证 → 判定终态 → （未收敛时）模型补丁 → 应用，直至退出条件。
async fn drive_rounds(
    task_id: i64,
    ctx: &ToolContext,
    provider: &dyn ModelProvider,
    config: &RepairConfig,
    sink: &dyn EventSink,
    deadline: Instant,
) -> Result<RepairResult> {
    let mut round: u32 = 0;
    loop {
        // 用户取消优先级最高：命中即退出，不进入新一轮。
        if is_cancelled(ctx, task_id)? {
            tracing::warn!(task_id, "任务已取消，退出修复循环");
            return Ok(RepairResult::Cancelled);
        }

        let failures = {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Ok(RepairResult::Timeout);
            };
            match tokio::time::timeout(remaining, run_all_verifies(task_id, ctx, config, sink))
                .await
            {
                Ok(inner) => inner?,
                Err(_elapsed) => {
                    tracing::warn!(task_id, "验证阶段超出总时限");
                    return Ok(RepairResult::Timeout);
                }
            }
        };
        if failures.is_empty() {
            return Ok(RepairResult::Success);
        }
        if round >= config.max_rounds {
            return Ok(RepairResult::Exceeded);
        }

        round += 1;
        sink.emit(XaimEvent::Repair { task_id, round })?;
        tracing::info!(task_id, round, failures = failures.len(), "进入修复轮");

        let diagnostics = render_diagnostics(&failures);
        let code = collect_code_context(&ctx.project_root, &failures);
        let task_text = {
            let conn = ctx.store.conn();
            store::task::get_task(&conn, task_id)?
                .map(|task| task.prompt)
                .unwrap_or_default()
        };
        let prompt = prompts::repair::build(&diagnostics, &code, &task_text);
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(RepairResult::Timeout);
        };
        let request = CompletionRequest::new(
            config.model.clone(),
            vec![
                Message::system(prompts::repair::SYSTEM),
                Message::user(prompt),
            ],
        );
        let response = match tokio::time::timeout(remaining, provider.complete(request)).await {
            Ok(result) => result?,
            Err(_elapsed) => {
                tracing::warn!(task_id, round, "模型调用超出总时限");
                return Ok(RepairResult::Timeout);
            }
        };
        sink.emit(XaimEvent::Token {
            task_id,
            input: i64::from(response.usage.input_tokens),
            output: i64::from(response.usage.output_tokens),
        })?;
        tracing::info!(
            task_id,
            round,
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            "模型已产出补丁"
        );

        if let Err(e) = patch::apply_diff(&response.content, &ctx.project_root) {
            // 应用失败视为本轮作废（模型输出问题），消耗一轮后继续。
            tracing::warn!(task_id, round, error = %e, "补丁应用失败");
        }
        // 回到循环头重新验证。
    }
}

/// 运行项目全部验证命令，逐条发 `Verify` 事件；返回失败条目（空 = 全部通过）。
async fn run_all_verifies(
    task_id: i64,
    ctx: &ToolContext,
    config: &RepairConfig,
    sink: &dyn EventSink,
) -> Result<Vec<VerifyFailure>> {
    let detected = detect_verify_config(&ctx.project_root);
    let commands = [
        detected.type_check.map(|c| (VerifyKind::TypeCheck, c)),
        detected.lint.map(|c| (VerifyKind::Lint, c)),
        detected.test.map(|c| (VerifyKind::Test, c)),
        detected.build.map(|c| (VerifyKind::Build, c)),
    ];

    let mut failures = Vec::new();
    for (kind, command) in commands.into_iter().flatten() {
        let result =
            runner::run_verify(kind, &command, &ctx.project_root, config.verify_timeout_sec)
                .await?;
        sink.emit(XaimEvent::Verify {
            task_id,
            kind: result.kind.clone(),
            passed: result.passed,
            output: result.output.clone(),
        })?;
        tracing::info!(
            task_id,
            kind = result.kind,
            passed = result.passed,
            command = %command,
            "验证完成"
        );
        if !result.passed {
            failures.push(VerifyFailure {
                kind,
                command,
                result,
            });
        }
    }
    Ok(failures)
}

/// 任务状态是否为已取消；任务不存在（如单测环境）视为未取消。
fn is_cancelled(ctx: &ToolContext, task_id: i64) -> Result<bool> {
    let conn = ctx.store.conn();
    let task = store::task::get_task(&conn, task_id)?;
    Ok(matches!(
        task.map(|t| t.status),
        Some(TaskStatus::Cancelled)
    ))
}

/// 依据验证命令选择对应的输出解析器。
fn parse_for_command(command: &str, output: &str) -> Vec<DiagError> {
    let cmd = command.trim_start();
    if cmd.starts_with("cargo ") {
        parser::parse_cargo(output)
    } else if cmd.starts_with("go ") {
        parser::parse_go(output)
    } else if cmd.contains("tsc") {
        parser::parse_tsc(output)
    } else if cmd.contains("eslint") {
        parser::parse_eslint(output)
    } else if cmd.contains("pytest") {
        parser::parse_pytest(output)
    } else {
        parser::parse_generic(output)
    }
}

/// 把失败条目渲染为进提示词的诊断文本：结构化诊断在前，原始输出（截断）在后。
fn render_diagnostics(failures: &[VerifyFailure]) -> String {
    let mut text = String::new();
    let mut diag_left = MAX_DIAGNOSTICS;
    for failure in failures {
        text.push_str(&format!(
            "\n## {} 未通过（命令 `{}`）\n",
            failure.kind.as_str(),
            failure.command
        ));
        for e in parse_for_command(&failure.command, &failure.result.output) {
            if diag_left == 0 {
                break;
            }
            diag_left -= 1;
            let location = if e.file.is_empty() {
                String::new()
            } else if e.line > 0 {
                format!("{}:{}:{}: ", e.file, e.line, e.column)
            } else {
                format!("{}: ", e.file)
            };
            text.push_str(&format!("- [{}] {}{}\n", e.severity, location, e.message));
        }
        text.push_str("原始输出：\n```\n");
        text.push_str(&truncate_middle(
            failure.result.output.trim_end(),
            MAX_RAW_OUTPUT_CHARS,
        ));
        text.push_str("\n```\n");
    }
    text
}

/// 从诊断里收集出错文件的相关代码：按诊断出现顺序对文件路径去重，逐个读取
/// 项目内文件内容（截断）渲染成 `=== 路径 ===` 摘要；一个都读不到（如诊断
/// 不带文件路径）时返回占位文案。
fn collect_code_context(root: &Path, failures: &[VerifyFailure]) -> String {
    let mut files: Vec<String> = Vec::new();
    for failure in failures {
        for e in parse_for_command(&failure.command, &failure.result.output) {
            if !e.file.is_empty() && !files.contains(&e.file) {
                files.push(e.file);
            }
        }
    }
    let mut digest = String::new();
    for file in files.iter().take(MAX_CODE_FILES) {
        let Ok(content) = fs::read_to_string(root.join(file)) else {
            continue;
        };
        digest.push_str(&format!(
            "\n=== {file} ===\n{}\n",
            truncate_middle(content.trim_end(), MAX_CODE_SNIPPET_CHARS)
        ));
    }
    if digest.is_empty() {
        return "（未能读取相关代码）".to_string();
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use crate::errors::XaimError;
    use crate::model::provider::MockProvider;
    use crate::store::Store;

    const BROKEN_LIB: &str = "pub fn add(a: i32, b: i32) -> i32 { a + }\n";
    const FIXED_LIB: &str = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";
    /// 模型产出：说明文字 + 代码围栏包裹的 diff，验证 parse_diff 的宽容解析。
    const FIX_DIFF: &str = "问题出在表达式未完成，修复如下：\n```diff\n\
                            --- a/src/lib.rs\n\
                            +++ b/src/lib.rs\n\
                            @@ -1 +1 @@\n\
                            -pub fn add(a: i32, b: i32) -> i32 { a + }\n\
                            +pub fn add(a: i32, b: i32) -> i32 { a + b }\n\
                            ```\n";
    /// 把错误 A 换成错误 B：第一轮能应用但修不好，同 diff 再次应用必然失败。
    const WORSEN_DIFF: &str = "--- a/src/lib.rs\n\
                               +++ b/src/lib.rs\n\
                               @@ -1 +1 @@\n\
                               -pub fn add(a: i32, b: i32) -> i32 { a + }\n\
                               +pub fn add(a: i32, b: i32) -> i32 { a + b + }\n";

    fn ctx_with(root: &Path) -> ToolContext {
        ToolContext {
            project_root: root.to_path_buf(),
            store: Arc::new(Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        }
    }

    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// 最小 cargo 项目；target/ 必须被忽略，否则回滚点提交会把构建产物全部收录。
    fn cargo_project(root: &Path, lib_body: &str) {
        write_file(root, ".gitignore", "/target\n");
        write_file(
            root,
            "Cargo.toml",
            "[package]\nname = \"xaim-fix-demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_file(root, "src/lib.rs", lib_body);
    }

    fn config(max_rounds: u32, total_timeout_sec: u64) -> RepairConfig {
        RepairConfig {
            max_rounds,
            verify_timeout_sec: 120,
            total_timeout_sec,
            model: "mock".to_string(),
        }
    }

    fn lib_content(root: &Path) -> String {
        fs::read_to_string(root.join("src/lib.rs")).unwrap()
    }

    /// 测试事件收集器：事件序列化为 JSON 存入内存，供可见性断言。
    #[derive(Default, Clone)]
    struct CollectSink(Arc<Mutex<Vec<serde_json::Value>>>);

    impl EventSink for CollectSink {
        fn emit(&self, event: XaimEvent) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(serde_json::to_value(&event).unwrap());
            Ok(())
        }
    }

    #[test]
    fn result_and_config_defaults_are_stable() {
        assert_eq!(RepairResult::Success.as_str(), "success");
        assert_eq!(RepairResult::Exceeded.as_str(), "exceeded");
        assert_eq!(RepairResult::Timeout.as_str(), "timeout");
        assert_eq!(RepairResult::Cancelled.as_str(), "cancelled");
        let config = RepairConfig::default();
        assert_eq!(config.max_rounds, 3);
        assert_eq!(config.verify_timeout_sec, 300);
        assert_eq!(config.model, "deepseek-reasoner");
    }

    #[test]
    fn parse_for_command_dispatches_by_tool() {
        let cargo_out = "error[E0308]: mismatched types\n --> src/main.rs:10:24\n";
        assert_eq!(
            parse_for_command("cargo check", cargo_out)[0].file,
            "src/main.rs"
        );
        assert_eq!(
            parse_for_command("go vet ./...", "./a.go:1:1: bad")[0].file,
            "./a.go"
        );
        assert_eq!(
            parse_for_command("pnpm exec tsc --noEmit", "a.ts(1,2): error TS1: x")[0].column,
            2
        );
        assert_eq!(
            parse_for_command("pnpm exec eslint .", "a.ts\n  1:1  error  x  r")[0].line,
            1
        );
        assert_eq!(
            parse_for_command("pytest", "FAILED t.py::test_a - boom")[0].file,
            "t.py"
        );
        assert_eq!(
            parse_for_command("mvn test", "App.java:10: error: ';' expected")[0].line,
            10
        );
    }

    #[test]
    fn render_diagnostics_keeps_location_and_raw_output() {
        let output = "error[E0308]: mismatched types\n --> src/main.rs:10:24\n";
        let failures = vec![VerifyFailure {
            kind: VerifyKind::TypeCheck,
            command: "cargo check".to_string(),
            result: VerifyResult {
                kind: "type_check".to_string(),
                passed: false,
                output: output.to_string(),
                duration_ms: 5,
            },
        }];
        let text = render_diagnostics(&failures);
        assert!(text.contains("type_check 未通过"), "{text}");
        assert!(text.contains("src/main.rs:10:24"), "{text}");
        assert!(text.contains("E0308: mismatched types"), "{text}");
        assert!(text.contains("原始输出"), "{text}");
    }

    #[test]
    fn collect_code_context_reads_diag_files_in_order() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "src/main.rs", "fn main() {}\n");
        write_file(dir.path(), "src/lib.rs", "pub mod x;\n");
        let failures = vec![
            VerifyFailure {
                kind: VerifyKind::TypeCheck,
                command: "cargo check".to_string(),
                result: VerifyResult {
                    kind: "type_check".to_string(),
                    passed: false,
                    output: "error[E0308]: bad\n --> src/main.rs:1:1\n".to_string(),
                    duration_ms: 1,
                },
            },
            VerifyFailure {
                kind: VerifyKind::Lint,
                command: "cargo clippy".to_string(),
                result: VerifyResult {
                    kind: "lint".to_string(),
                    passed: false,
                    // main.rs 再次出现用于验证去重；lib.rs 验证跨失败项收集。
                    output: "warning: style\n \
                             --> src/main.rs:2:1\nwarning: other\n \
                             --> src/lib.rs:1:1\n"
                        .to_string(),
                    duration_ms: 1,
                },
            },
        ];

        let digest = collect_code_context(dir.path(), &failures);
        assert!(digest.contains("=== src/main.rs ==="), "{digest}");
        assert!(digest.contains("fn main()"), "{digest}");
        assert!(digest.contains("=== src/lib.rs ==="), "{digest}");
        // 同一文件重复出现在多条诊断里时只保留一份。
        assert_eq!(digest.matches("=== ").count(), 2, "文件应去重: {digest}");

        // 诊断不带文件路径时退回占位文案。
        let no_files = vec![VerifyFailure {
            kind: VerifyKind::Test,
            command: "cargo test".to_string(),
            result: VerifyResult {
                kind: "test".to_string(),
                passed: false,
                output: "test failed".to_string(),
                duration_ms: 1,
            },
        }];
        assert_eq!(
            collect_code_context(dir.path(), &no_files),
            "（未能读取相关代码）"
        );
    }

    #[tokio::test]
    async fn success_when_verification_passes_on_first_sweep() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), FIXED_LIB);
        let ctx = ctx_with(dir.path());
        let sink = CollectSink::default();

        let result = repair_loop_with_sink(
            1,
            &ctx,
            &MockProvider::new("不应被调用"),
            &config(3, 600),
            &sink,
        )
        .await
        .unwrap();

        assert_eq!(result, RepairResult::Success);
        assert_eq!(lib_content(dir.path()), FIXED_LIB);
    }

    #[tokio::test]
    async fn repairs_within_one_round() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let ctx = ctx_with(dir.path());
        let sink = CollectSink::default();

        let result = repair_loop_with_sink(
            1,
            &ctx,
            &MockProvider::new(FIX_DIFF),
            &config(3, 600),
            &sink,
        )
        .await
        .unwrap();

        assert_eq!(result, RepairResult::Success);
        assert_eq!(lib_content(dir.path()), FIXED_LIB);
    }

    #[tokio::test]
    async fn exceeds_rounds_and_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let ctx = ctx_with(dir.path());
        let sink = CollectSink::default();

        let result = repair_loop_with_sink(
            1,
            &ctx,
            &MockProvider::new(WORSEN_DIFF),
            &config(3, 600),
            &sink,
        )
        .await
        .unwrap();

        assert_eq!(result, RepairResult::Exceeded);
        // 三轮耗尽后回滚：文件恢复为任务前的错误 A，而非修复尝试的产物。
        assert_eq!(lib_content(dir.path()), BROKEN_LIB);
    }

    #[tokio::test]
    async fn each_round_is_visible_via_events() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let ctx = ctx_with(dir.path());
        let sink = CollectSink::default();

        let result = repair_loop_with_sink(
            7,
            &ctx,
            &MockProvider::new(WORSEN_DIFF),
            &config(2, 600),
            &sink,
        )
        .await
        .unwrap();
        assert_eq!(result, RepairResult::Exceeded);

        let events = sink.0.lock().unwrap();
        let repair_rounds: Vec<u32> = events
            .iter()
            .filter(|e| e["type"] == "repair")
            .filter_map(|e| e["round"].as_u64().map(|r| r as u32))
            .collect();
        assert_eq!(
            repair_rounds,
            vec![1, 2],
            "每轮都应发 Repair 事件: {events:?}"
        );
        assert!(events
            .iter()
            .any(|e| e["type"] == "verify" && e["taskId"] == 7));
        assert!(events
            .iter()
            .any(|e| e["type"] == "token" && e["taskId"] == 7));
    }

    #[tokio::test]
    async fn total_timeout_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let script = if cfg!(windows) {
            "ping -n 4 127.0.0.1"
        } else {
            "sleep 4"
        };
        write_file(
            dir.path(),
            "package.json",
            &format!("{{\"name\": \"slow\", \"scripts\": {{\"test\": \"{script}\"}}}}"),
        );
        let ctx = ctx_with(dir.path());
        let sink = CollectSink::default();

        let result =
            repair_loop_with_sink(1, &ctx, &MockProvider::new("unused"), &config(3, 1), &sink)
                .await
                .unwrap();

        assert_eq!(result, RepairResult::Timeout);
        assert!(
            dir.path().join("package.json").is_file(),
            "回滚后原文件仍在"
        );
    }

    #[tokio::test]
    async fn cancelled_task_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let ctx = ctx_with(dir.path());
        let sink = CollectSink::default();

        // 在库中登记任务并标记取消：循环应立即退出且不调用模型。
        let task_id = {
            let conn = ctx.store.conn();
            let project_id =
                store::project::insert_project(&conn, &dir.path().display().to_string(), "demo")
                    .unwrap();
            let task_id = store::task::insert_task(&conn, project_id, "fix it").unwrap();
            store::task::update_task_status(&conn, task_id, TaskStatus::Cancelled).unwrap();
            task_id
        };

        let result = repair_loop_with_sink(
            task_id,
            &ctx,
            &MockProvider::new(FIX_DIFF),
            &config(3, 600),
            &sink,
        )
        .await
        .unwrap();

        assert_eq!(result, RepairResult::Cancelled);
        assert_eq!(lib_content(dir.path()), BROKEN_LIB, "取消后不应有补丁落地");
    }

    #[tokio::test]
    async fn model_failure_propagates_and_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let ctx = ctx_with(dir.path());
        let sink = CollectSink::default();

        let provider = MockProvider::new("ignored").with_error("429 限流");
        let err = repair_loop_with_sink(1, &ctx, &provider, &config(3, 600), &sink)
            .await
            .unwrap_err();

        assert!(matches!(err, XaimError::Model(ref m) if m == "429 限流"));
        assert_eq!(lib_content(dir.path()), BROKEN_LIB, "异常出口同样回滚");
    }
}
