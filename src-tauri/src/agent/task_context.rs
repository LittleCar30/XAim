//! Program-owned checkpoints. File contents stay on disk and in the durable
//! change cache; rebuilding the prompt never needs another model request.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::tool_loop::Recorder;
use super::workspace::{ChangeSet, TaskWorkspace};
use crate::errors::{Result, XaimError};
use crate::model::provider::ToolMessage;
use crate::store;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    #[serde(default)]
    pub task_kind: String,
    pub goal: String,
    pub phase: String,
    pub round: usize,
    pub revision: usize,
    pub input_used: u64,
    pub output_used: u64,
    pub remaining_tokens: u64,
    pub predicted_rounds: u64,
    pub files: Vec<Value>,
    pub completed_operations: BTreeMap<String, usize>,
    pub recent_errors: Vec<Value>,
    /// Model assertions are deliberately separate from observed tool results.
    pub reported_completed: Vec<String>,
    pub pending: Vec<String>,
    pub ready_for_review: bool,
    pub model_summary: String,
    #[serde(default)]
    pub reported_pending: bool,
    pub verification: Vec<Value>,
    pub verified_revision: Option<usize>,
    #[serde(default)]
    pub operations: Vec<Value>,
    #[serde(default)]
    pub required_operation: Option<String>,
    #[serde(default)]
    pub required_operations: Vec<String>,
}

/// Recognize explicit prohibitions immediately before an operation phrase.
/// Check each occurrence so a later affirmative instruction still counts.
fn has_unnegated_phrase(goal: &str, phrase: &str) -> bool {
    goal.match_indices(phrase).any(|(offset, _)| {
        let prefix = goal[..offset].trim_end();
        ![
            "do not",
            "don't",
            "don’t",
            "must not",
            "should not",
            "never",
            "no need to",
            "don't need to",
            "无需",
            "不需要",
            "不用",
            "不必",
            "不要",
            "不得",
            "禁止",
            "请勿",
            "勿",
            "别",
        ]
        .iter()
        .any(|negation| {
            prefix.strip_suffix(negation).is_some_and(|before| {
                !negation.is_ascii()
                    || before
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_ascii_alphanumeric())
            })
        })
    })
}

fn operation_goals(goal: &str) -> Vec<String> {
    let lower = goal.to_ascii_lowercase();
    let discussion = [
        "解释",
        "说明",
        "介绍",
        "告诉我",
        "如何",
        "怎么",
        "explain ",
        "describe ",
        "how ",
    ]
    .iter()
    .any(|w| lower.trim_start_matches('请').starts_with(w));
    if (discussion || is_analysis_goal(goal))
        && !["然后", "并运行", "并执行", "then "]
            .iter()
            .any(|w| lower.contains(w))
    {
        return vec![];
    }
    // Implementing an operation tool is a coding task, not an instruction to use it.
    if [
        "实现",
        "编写",
        "功能",
        "按钮",
        "implement",
        "write a",
        "create a",
        "生成脚本",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        return vec![];
    }
    let mut goals = Vec::new();
    if has_unnegated_phrase(&lower, "克隆")
        || lower.starts_with("clone ")
        || lower.starts_with("git clone ")
    {
        goals.push("clone_repository".into());
    }
    for (kind, words) in [
        (
            "install",
            vec!["安装依赖", "安装项目依赖", "install dependencies"],
        ),
        (
            "test",
            vec!["运行测试", "执行测试", "run tests", "run the tests"],
        ),
        (
            "build",
            vec!["构建项目", "build project", "build the project"],
        ),
        (
            "start_process",
            vec!["启动", "start server", "start the server"],
        ),
        ("run_command", vec!["执行命令", "运行脚本", "execute "]),
    ] {
        if words.iter().any(|word| has_unnegated_phrase(&lower, word)) {
            goals.push(kind.into());
        }
    }
    goals
}

fn matches_operation(kind: &str, evidence: &Value) -> bool {
    let tool = evidence["tool"].as_str().unwrap_or_default();
    if kind == "clone_repository" {
        return tool == kind;
    }
    if kind == "start_process" {
        return matches!(tool, "read_process" | "stop_process");
    }
    if tool == "run_verify" {
        return matches!(kind, "test" | "build")
            && evidence["result"].as_array().is_some_and(|checks| {
                checks
                    .iter()
                    .any(|c| c["kind"].as_str().is_some_and(|name| name.contains(kind)))
            });
    }
    if tool != "run_command" {
        return false;
    }
    if kind == "run_command" {
        return true;
    }
    let result = &evidence["result"];
    let program = result["program"].as_str().unwrap_or_default();
    let args: Vec<&str> = result["args"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    match kind {
        "install" => {
            (matches!(program, "npm" | "pnpm" | "yarn")
                && args
                    .first()
                    .is_some_and(|a| matches!(*a, "install" | "ci" | "add")))
                || (program.contains("python")
                    && args.windows(3).any(|w| w == ["-m", "pip", "install"]))
                || (program == "cargo" && args.first() == Some(&"fetch"))
        }
        "test" | "build" => {
            let script = if args
                .first()
                .is_some_and(|a| matches!(*a, "run" | "run-script"))
            {
                args.get(1)
            } else {
                args.first()
            };
            (matches!(program, "npm" | "pnpm" | "yarn" | "cargo" | "go")
                && script.is_some_and(|s| *s == kind || s.starts_with(&format!("{kind}:"))))
                || (kind == "test"
                    && (program == "pytest"
                        || program.contains("python")
                            && args
                                .windows(2)
                                .any(|w| w == ["-m", "pytest"] || w == ["-m", "unittest"])
                        || matches!(
                            program,
                            "node" | "python" | "python3" | "sh" | "bash" | "powershell" | "pwsh"
                        ) && args.iter().any(|a| {
                            a.ends_with("test.js")
                                || a.ends_with("test.py")
                                || a.ends_with("test.sh")
                                || a.ends_with("test.ps1")
                        })))
                || (kind == "build" && program == "make")
        }
        _ => false,
    }
}

fn is_analysis_goal(goal: &str) -> bool {
    let lower = goal.to_ascii_lowercase();
    let matches = |word: &&str| {
        if word.is_ascii() {
            lower
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|token| token == *word)
        } else {
            goal.contains(*word)
        }
    };
    if [
        "修复",
        "修改",
        "实现",
        "创建",
        "新增",
        "添加",
        "重构",
        "生成",
        "构建",
        "删除",
        "更新",
        "编写",
        "增加",
        "fix",
        "implement",
        "create",
        "add",
        "refactor",
        "build",
        "write",
        "update",
        "change",
        "delete",
        "generate",
    ]
    .iter()
    .any(matches)
    {
        return false;
    }
    [
        "介绍",
        "分析",
        "解释",
        "说明",
        "审查",
        "评审",
        "对比",
        "总结",
        "为什么",
        "如何",
        "什么",
        "describe",
        "explain",
        "analyze",
        "review",
        "compare",
        "summarize",
        "why",
        "how",
    ]
    .iter()
    .any(matches)
}

impl TaskSnapshot {
    pub fn operation_satisfied(&self) -> bool {
        if !self.required_operations.is_empty() {
            return self.required_operations.iter().all(|kind| {
                if kind == "start_process" {
                    let Some(started) = self
                        .operations
                        .iter()
                        .rev()
                        .find(|v| v["tool"] == "start_process")
                    else {
                        return false;
                    };
                    return started["success"] == true
                        && self
                            .operations
                            .iter()
                            .rev()
                            .find(|v| {
                                matches!(v["tool"].as_str(), Some("read_process" | "stop_process"))
                                    && v["result"]["process_id"] == started["result"]["process_id"]
                            })
                            .is_some_and(|v| {
                                v["success"] == true && v["result"]["status"]["state"] == "running"
                            });
                }
                self.operations
                    .iter()
                    .rev()
                    .find(|v| matches_operation(kind, v))
                    .is_some_and(|v| {
                        v["success"] == true
                            && (kind != "start_process"
                                || v["result"]["status"]["state"] == "running")
                    })
            });
        }
        self.task_kind != "operation"
            || self
                .operations
                .iter()
                .rev()
                .find(|v| matches!(v["tool"].as_str(), Some("clone_repository" | "run_command")))
                .is_some_and(|v| v["success"] == true)
    }

    pub fn needs_command_permission(&self, allow_verification: bool) -> bool {
        self.required_operations
            .iter()
            .any(|kind| !allow_verification || !matches!(kind.as_str(), "test" | "build"))
    }

    pub fn new(goal: &str) -> Self {
        Self {
            task_kind: if !operation_goals(goal).is_empty() {
                "operation".into()
            } else if is_analysis_goal(goal) {
                "analysis".into()
            } else {
                "coding".into()
            },
            goal: goal.into(),
            phase: "executing".into(),
            round: 0,
            revision: 0,
            input_used: 0,
            output_used: 0,
            remaining_tokens: 0,
            predicted_rounds: 0,
            files: vec![],
            completed_operations: BTreeMap::new(),
            recent_errors: vec![],
            reported_completed: vec![],
            pending: vec!["原始用户要求的完成情况尚未确认".into()],
            ready_for_review: false,
            model_summary: String::new(),
            reported_pending: false,
            verification: vec![],
            verified_revision: None,
            operations: vec![],
            required_operation: operation_goals(goal).first().cloned(),
            required_operations: operation_goals(goal),
        }
    }

    pub fn observe(
        &mut self,
        name: &str,
        path: Option<&str>,
        success: bool,
        changed: bool,
        error: Option<&str>,
    ) {
        if success {
            let key = format!("{name}{}", path.map_or(String::new(), |p| format!(":{p}")));
            *self.completed_operations.entry(key).or_default() += 1;
            if changed {
                self.revision += 1;
                self.ready_for_review = false;
                self.verified_revision = None;
            }
        } else {
            self.ready_for_review = false;
            self.recent_errors.push(json!({"tool":name,"path":path,
                "error":error.unwrap_or("工具未成功").chars().take(2048).collect::<String>()}));
            if self.recent_errors.len() > 8 {
                self.recent_errors.remove(0);
            }
        }
    }

    pub fn update_reported_state(&mut self, args: Value) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Report {
            #[serde(default)]
            task_kind: String,
            completed: Vec<String>,
            pending: Vec<String>,
            ready_for_review: bool,
            summary: String,
        }
        let report: Report = serde_json::from_value(args)?;
        if report.completed.len() > 16
            || report.pending.len() > 16
            || report
                .completed
                .iter()
                .chain(&report.pending)
                .any(|s| s.chars().count() > 2048)
            || report.summary.chars().count() > 32_000
            || !matches!(
                report.task_kind.as_str(),
                "" | "analysis" | "coding" | "operation"
            )
            || (report.ready_for_review
                && (!report.pending.is_empty()
                    || report.completed.is_empty()
                    || report.summary.trim().is_empty()))
        {
            return Err(XaimError::Config(
                "任务状态过长，或仍有待办却声明完成".into(),
            ));
        }
        self.reported_completed = report.completed;
        self.reported_pending = !report.pending.is_empty();
        if !report.task_kind.is_empty() && self.required_operation.is_none() {
            self.task_kind = report.task_kind;
        }
        self.pending = report.pending;
        self.ready_for_review = report.ready_for_review;
        self.model_summary = report.summary;
        Ok(())
    }

    pub fn save(&mut self, workspace: &TaskWorkspace, record: &Recorder<'_>) -> Result<ChangeSet> {
        let started = std::time::Instant::now();
        let changes = workspace.changes()?;
        self.files = changes.files.iter().map(|f| json!({
            "path":f.path,"deleted":f.after.is_none(),
            "hash":f.after.as_ref().map(|s| blake3::hash(s.as_bytes()).to_hex().to_string()),
            "bytes":f.after.as_ref().map_or(0, String::len),
        })).collect();
        let conn = record.store.conn();
        let tx = conn.unchecked_transaction()?;
        store::cache::set_cache(
            &tx,
            &format!("task:{}:snapshot", record.task_id),
            &serde_json::to_string(self)?,
            None,
        )?;
        // Save after every successful write, before a subsequent model call.
        store::cache::set_cache(
            &tx,
            &format!("task:{}:partial_changes", record.task_id),
            &serde_json::to_string(&changes)?,
            None,
        )?;
        tx.commit()?;
        drop(conn);
        record.diagnostic(
            "checkpointPersistence",
            json!({"mode":"files","revision":self.revision,
            "durationMs":started.elapsed().as_millis() as u64,"changedFiles":changes.files.len()}),
        )?;
        Ok(changes)
    }

    /// Read/search/state tools cannot modify task files. Preserve the last durable
    /// candidate and save only metadata; writes and final reports still scan files.
    pub fn save_metadata(&self, record: &Recorder<'_>) -> Result<()> {
        let started = std::time::Instant::now();
        store::cache::set_cache(
            &record.store.conn(),
            &format!("task:{}:snapshot", record.task_id),
            &serde_json::to_string(self)?,
            None,
        )?;
        record.diagnostic(
            "checkpointPersistence",
            json!({"mode":"metadata","revision":self.revision,
            "durationMs":started.elapsed().as_millis() as u64}),
        )
    }

    pub fn save_report(
        &mut self,
        workspace: &TaskWorkspace,
        record: &Recorder<'_>,
        reason: &str,
    ) -> Result<String> {
        let changes = self.save(workspace, record)?;
        let verification_status =
            if matches!(self.task_kind.as_str(), "analysis" | "operation") && self.revision == 0 {
                "not_required"
            } else if self.verified_revision != Some(self.revision) {
                "not_run"
            } else if self.verification.iter().any(|r| r["status"] == "failed") {
                "failed"
            } else if self.verification.is_empty()
                || self
                    .verification
                    .iter()
                    .any(|r| r["status"] == "unavailable")
            {
                "unavailable"
            } else {
                "passed"
            };
        let report = json!({"reason":reason,"phase":self.phase,"files":self.files,
            "diff":changes.diff(),"verificationStatus":verification_status,
            "verification":self.verification,"pending":self.pending,
            "operations":self.operations,
            "completionConfirmed":self.ready_for_review && self.operation_satisfied()});
        store::cache::set_cache(
            &record.store.conn(),
            &format!("task:{}:report", record.task_id),
            &report.to_string(),
            None,
        )?;
        let paths = changes
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>()
            .join("、");
        Ok(format!(
            "{reason}\n程序汇总：{} 个变更文件{}；验证状态：{}。",
            changes.files.len(),
            if paths.is_empty() {
                String::new()
            } else {
                format!("（{paths}）")
            },
            match verification_status {
                "not_required" => "不适用（分析任务）",
                "passed" => "通过",
                "failed" => "检查失败",
                "unavailable" => "无法验证",
                _ => "尚未检查",
            }
        ))
    }

    /// A checkpoint starts a fresh conversation; no surviving assistant message
    /// has its provider-required reasoning removed or its tool results orphaned.
    pub fn rebuild(&self, messages: &mut Vec<ToolMessage>, recent_budget: usize) -> Result<bool> {
        let mut pending = HashSet::new();
        for m in messages.iter() {
            for call in &m.tool_calls {
                if !pending.insert(call.id.as_str()) {
                    return Ok(false);
                }
            }
            if m.role == "tool" {
                let Some(id) = m.tool_call_id.as_deref() else {
                    return Ok(false);
                };
                if !pending.remove(id) {
                    return Ok(false);
                }
            }
        }
        if !pending.is_empty() || messages.len() <= 2 {
            return Ok(false);
        }
        let mut checkpoint = serde_json::to_value(self)?;
        checkpoint.as_object_mut().unwrap().remove("goal");
        // Long check output is already durable. Keep a bounded diagnostic excerpt.
        if let Some(checks) = checkpoint["verification"].as_array_mut() {
            for check in checks {
                if let Some(output) = check["output"].as_str() {
                    check["output"] = json!(output.chars().take(4000).collect::<String>());
                }
            }
        }
        let replacement = ToolMessage::text("user", format!(
            "程序任务检查点（不是新任务）。目标仍以上一条用户要求为准。已完成工具操作是事实；reportedCompleted、pending、modelSummary 是模型声明，不能当作验证通过。检查点摘要不含文件正文；优先使用后续保留的工具结果，仅在所需正文缺失时按路径读取。继续完成 pending；若存在真实 failed 检查只修复相关问题；unavailable 不代表代码错误。\n{}", checkpoint));
        // Keep the latest complete tool group. In particular, never checkpoint
        // away a fresh read: the next request must be able to use its contents.
        // Whole groups preserve provider-required reasoning and tool pairing.
        let tail = messages
            .iter()
            .rposition(|m| m.role == "assistant")
            .map(|start| &messages[start..]);
        let recent = match tail {
            Some(tail)
                if crate::utils::tokens::estimate_tokens(&serde_json::to_string(tail)?)
                    <= recent_budget =>
            {
                tail.to_vec()
            }
            Some(tail)
                if tail.iter().any(|m| {
                    m.tool_calls.iter().any(|c| {
                        matches!(
                            c.function.name.as_str(),
                            "read_file"
                                | "read_files"
                                | "read_symbol"
                                | "search"
                                | "search_symbols"
                                | "list_dir"
                        )
                    })
                }) =>
            {
                return Ok(false);
            }
            _ => vec![],
        };
        *messages = vec![messages[0].clone(), messages[1].clone(), replacement];
        messages.extend(recent);
        Ok(true)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BudgetForecast {
    pub cost_per_round: u64,
    pub rounds: u64,
}

pub fn forecast(
    remaining: u64,
    input: usize,
    recent_output: u64,
    normal_output: u32,
) -> BudgetForecast {
    let reasonable_output = recent_output
        .max(1024)
        .min(normal_output.clamp(1, 8192) as u64);
    let cost_per_round = (input as u64).saturating_add(reasonable_output).max(1);
    BudgetForecast {
        cost_per_round,
        rounds: remaining / cost_per_round,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn operation_requirements_need_matching_latest_evidence() {
        let mut snapshot = TaskSnapshot::new("克隆仓库，安装依赖并运行测试，然后启动服务");
        assert_eq!(
            snapshot.required_operations,
            ["clone_repository", "install", "test", "start_process"]
        );
        snapshot.operations.push(json!({"tool":"run_command","success":true,"result":{"program":"node","args":["--version"]}}));
        assert!(!snapshot.operation_satisfied());
        snapshot.operations.extend([
            json!({"tool":"clone_repository","success":true}),
            json!({"tool":"run_command","success":true,"result":{"program":"npm","args":["ci"]}}),
            json!({"tool":"run_command","success":true,"result":{"program":"npm","args":["test"]}}),
            json!({"tool":"start_process","success":true,"result":{"process_id":"service"}}),
        ]);
        assert!(!snapshot.operation_satisfied());
        snapshot.operations.push(
            json!({"tool":"read_process","success":true,"result":{"process_id":"service","status":{"state":"running"}}}),
        );
        assert!(snapshot.operation_satisfied());
        snapshot.operations.push(json!({"tool":"run_command","success":false,"result":{"program":"npm","args":["test"]}}));
        assert!(!snapshot.operation_satisfied());
    }

    #[test]
    fn discussing_operations_does_not_require_execution_permission() {
        for goal in [
            "解释如何克隆仓库",
            "介绍如何安装依赖和启动服务",
            "实现克隆工具",
            "explain how to run tests",
            "解释如何构建项目",
        ] {
            assert!(
                TaskSnapshot::new(goal).required_operations.is_empty(),
                "{goal}"
            );
        }
        assert!(!TaskSnapshot::new("运行测试").needs_command_permission(true));
        assert!(TaskSnapshot::new("安装依赖并运行测试").needs_command_permission(true));
        assert!(!TaskSnapshot::new("使用工具克隆仓库")
            .required_operations
            .is_empty());
    }

    #[test]
    fn explicitly_negated_operations_are_constraints_instead_of_requirements() {
        for goal in [
            "Create index.html. Do not install dependencies.",
            "Update index.html. Don't run the tests.",
            "Never start the server.",
            "No need to build the project.",
            "创建页面，不要安装依赖。",
            "更新页面，请勿运行测试。",
            "不需要启动服务。",
            "不要克隆仓库。",
            "无需执行命令。",
        ] {
            let snapshot = TaskSnapshot::new(goal);
            assert!(snapshot.required_operations.is_empty(), "{goal}");
            assert!(!snapshot.needs_command_permission(false), "{goal}");
        }
        assert_eq!(
            TaskSnapshot::new("不要安装依赖，运行测试。").required_operations,
            ["test"]
        );
        assert_eq!(
            TaskSnapshot::new("Do not install dependencies. Run tests.").required_operations,
            ["test"]
        );
        assert_eq!(
            TaskSnapshot::new("Do not run tests yet. Then run tests.").required_operations,
            ["test"]
        );
        assert_eq!(
            TaskSnapshot::new("不要克隆仓库，安装依赖。").required_operations,
            ["install"]
        );
    }
    use super::*;
    use crate::model::provider::{FunctionCall, ToolCall};

    #[test]
    fn checkpoint_keeps_large_fresh_reads_and_refuses_to_discard_them() {
        for tool_name in ["read_file", "read_files"] {
            let snapshot = TaskSnapshot::new("Finish page");
            let body = "fresh SVG file\n".repeat(1500);
            let mut read = ToolMessage::text("assistant", "");
            read.tool_calls = vec![ToolCall {
                id: "read".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: tool_name.into(),
                    arguments: "{\"path\":\"index.html\"}".into(),
                },
            }];
            let messages = vec![
                ToolMessage::text("system", "stable"),
                ToolMessage::text("user", "Finish page"),
                ToolMessage::text("assistant", "old history"),
                read,
                ToolMessage::result("read", body.clone()),
            ];
            let mut too_small = messages.clone();
            assert!(!snapshot.rebuild(&mut too_small, 2000).unwrap());
            assert_eq!(too_small.len(), messages.len());
            let mut enough = messages;
            assert!(snapshot.rebuild(&mut enough, 16_384).unwrap());
            assert_eq!(
                enough.last().unwrap().content.as_deref(),
                Some(body.as_str())
            );
            assert_eq!(enough[3].tool_calls[0].id, "read");
            assert_eq!(enough[4].tool_call_id.as_deref(), Some("read"));
        }
    }

    #[test]
    fn checkpoint_refuses_open_tool_pairs_and_only_replaces_closed_history() {
        let snapshot = TaskSnapshot::new("Create page");
        let mut assistant = ToolMessage::text("assistant", "");
        assistant.reasoning_content = Some("private".repeat(10_000));
        assistant.tool_calls = vec![ToolCall {
            id: "a".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "create_file".into(),
                arguments: "{}".into(),
            },
        }];
        let mut messages = vec![
            ToolMessage::text("system", "stable"),
            ToolMessage::text("user", "Create page"),
            assistant,
        ];
        assert!(!snapshot.rebuild(&mut messages, 2000).unwrap());
        assert!(messages[2].reasoning_content.is_some());
        messages.push(ToolMessage::result("a", "done".into()));
        assert!(snapshot.rebuild(&mut messages, 2000).unwrap());
        assert_eq!(messages[0].content.as_deref(), Some("stable"));
        assert_eq!(messages[1].content.as_deref(), Some("Create page"));
        assert!(messages.iter().all(|m| m.tool_calls.is_empty()
            && m.tool_call_id.is_none()
            && m.reasoning_content.is_none()));
    }
}
