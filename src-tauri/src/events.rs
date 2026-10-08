//! 后端推送给前端的事件流（单一事件通道 [`EVENT_CHANNEL`]）。
//! payload 与前端 `src/types/index.ts` 的 `XaimEvent` 判别联合一一对应：
//! `type` 标签为变体名的 camelCase，字段名同为 camelCase。

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::errors::{Result, XaimError};

/// 所有后端 → 前端事件共用的通道名。
pub const EVENT_CHANNEL: &str = "xaim://event";

/// 后端事件。序列化形如 `{"type":"step","taskId":1,"name":"...","status":"..."}`。
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum XaimEvent {
    /// 任务步骤推进。
    Step {
        task_id: i64,
        name: String,
        status: String,
    },
    /// 工具调用。
    Tool {
        task_id: i64,
        tool: String,
        args: String,
    },
    /// 补丁产出。
    Patch {
        task_id: i64,
        file: String,
        diff: String,
    },
    /// 验证结果。
    Verify {
        task_id: i64,
        kind: String,
        passed: bool,
        output: String,
    },
    /// 修复回路进入第 round 轮。
    Repair { task_id: i64, round: u32 },
    /// token 用量增量。
    Token {
        task_id: i64,
        input: i64,
        output: i64,
    },
    /// 任务进入终态。
    Done { task_id: i64, status: String },
    /// 错误。
    Error { task_id: i64, message: String },
}

/// 向所有窗口推送事件。推送失败不 panic，经统一错误类型返回。
pub fn emit(app: &AppHandle, event: XaimEvent) -> Result<()> {
    app.emit(EVENT_CHANNEL, event)
        .map_err(|e| XaimError::Unknown(format!("事件推送失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn step_serializes_to_frontend_shape() {
        let event = XaimEvent::Step {
            task_id: 1,
            name: "生成补丁".to_string(),
            status: "running".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({"type": "step", "taskId": 1, "name": "生成补丁", "status": "running"})
        );
    }

    #[test]
    fn every_variant_has_camel_case_tag_and_fields() {
        let cases = [
            (
                XaimEvent::Step {
                    task_id: 7,
                    name: "n".to_string(),
                    status: "running".to_string(),
                },
                "step",
            ),
            (
                XaimEvent::Tool {
                    task_id: 7,
                    tool: "read_file".to_string(),
                    args: "{}".to_string(),
                },
                "tool",
            ),
            (
                XaimEvent::Patch {
                    task_id: 7,
                    file: "a.rs".to_string(),
                    diff: "d".to_string(),
                },
                "patch",
            ),
            (
                XaimEvent::Verify {
                    task_id: 7,
                    kind: "cargo_test".to_string(),
                    passed: true,
                    output: "ok".to_string(),
                },
                "verify",
            ),
            (
                XaimEvent::Repair {
                    task_id: 7,
                    round: 2,
                },
                "repair",
            ),
            (
                XaimEvent::Token {
                    task_id: 7,
                    input: 10,
                    output: 5,
                },
                "token",
            ),
            (
                XaimEvent::Done {
                    task_id: 7,
                    status: "done".to_string(),
                },
                "done",
            ),
            (
                XaimEvent::Error {
                    task_id: 7,
                    message: "m".to_string(),
                },
                "error",
            ),
        ];
        assert_eq!(cases.len(), 8);
        for (event, tag) in cases {
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(value["type"], tag);
            assert_eq!(value["taskId"], 7);
        }
    }

    #[test]
    fn verify_variant_keeps_field_names_and_types() {
        let event = XaimEvent::Verify {
            task_id: 3,
            kind: "cargo_test".to_string(),
            passed: false,
            output: "test failed".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({"type": "verify", "taskId": 3, "kind": "cargo_test", "passed": false, "output": "test failed"})
        );
    }
}
