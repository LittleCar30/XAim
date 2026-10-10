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
    pub fn new(goal: &str) -> Self {
        Self {
            task_kind: if is_analysis_goal(goal) {
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
            if changed && matches!(name, "create_file" | "append_file" | "edit_file") {
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
            || !matches!(report.task_kind.as_str(), "" | "analysis" | "coding")
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
        if !report.task_kind.is_empty() {
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
        let verification_status = if self.task_kind == "analysis" && self.revision == 0 {
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
            "completionConfirmed":self.ready_for_review});
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
