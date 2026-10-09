//! 后端与前端 IPC 之间的共享领域类型（统一数据契约）。
//!
//! 约定：
//! - 所有结构体序列化为 camelCase JSON（`#[serde(rename_all = "camelCase")]`）；
//! - 时间戳一律为毫秒级 `i64`；
//! - 可选字段用 `Option<T>`，序列化为 `null`；
//! - 温度用 `f32`，其余浮点（cost/score）用 `f64`。

use serde::{Deserialize, Serialize};

/// 任务生命周期状态（共 15 个），JSON 取值形如 "idle"、"applyingPatch"、"waitingConfirm"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Idle,
    Parsing,
    Indexing,
    Retrieving,
    Planning,
    Generating,
    ApplyingPatch,
    Verifying,
    Repairing,
    WaitingConfirm,
    Applied,
    Done,
    Failed,
    RolledBack,
    Cancelled,
}

/// 项目元信息，对应 projects 表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub created_at: i64,
    pub last_opened_at: i64,
}

/// 一次编码任务及其统计信息，对应 tasks 表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: i64,
    pub project_id: i64,
    pub prompt: String,
    pub status: TaskStatus,
    pub intent: Option<String>,
    pub plan: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub token_input: Option<i64>,
    pub token_output: Option<i64>,
    pub cost: Option<f64>,
}

/// 任务执行流水中的单步记录（意图解析、检索、生成等），对应 task_steps 表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStep {
    pub id: i64,
    pub task_id: i64,
    pub step_type: String,
    pub content: String,
    pub created_at: i64,
}

/// 一份候选补丁，对应 patches 表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    pub id: i64,
    pub task_id: i64,
    pub file_path: String,
    pub diff: String,
    pub applied: bool,
    pub score: Option<f64>,
    pub created_at: i64,
}

/// 文件索引记录，对应 files 表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRecord {
    pub id: i64,
    pub project_id: i64,
    pub path: String,
    pub language: Option<String>,
    pub size: Option<i64>,
    pub mtime: Option<i64>,
    pub hash: Option<String>,
    pub summary: Option<String>,
    pub indexed_at: Option<i64>,
}

/// 单项验证（构建/测试/静态检查）的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    pub kind: String,
    pub passed: bool,
    pub output: String,
    pub duration_ms: i64,
}

/// 模型接入配置，对应 config 存储中的单个模型条目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub provider: String,
    pub name: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: "deepseek".to_string(),
            name: "deepseek-chat".to_string(),
            api_key: None,
            base_url: None,
            temperature: 0.7,
            max_tokens: None,
        }
    }
}

/// 应用配置（IPC 契约）：模型接入与任务路由。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// 模型接入配置。
    pub model: ModelConfig,
    /// 任务 → 模型路由配置。
    pub router: crate::model::router::RouterConfig,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub model_profiles: Vec<ModelProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_model_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfile {
    pub id: String,
    pub label: String,
    pub config: ModelConfig,
    #[serde(default)]
    pub supports_reasoning: bool,
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub thinking: Option<bool>,
}

/// 文件树节点，`children` 仅目录非空；文件节点 `size` 为字节数，目录为 `null`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub children: Vec<FileNode>,
}

/// 进入模型上下文的文件片段及入选原因与 token 估算。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextFile {
    pub path: String,
    pub content: String,
    pub reason: String,
    pub tokens: i64,
}

/// 可跳转的代码符号（函数/结构体等）及其行区间。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub start_line: i64,
    pub end_line: i64,
}

/// 一次工具调用的结构化结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub name: String,
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
}

/// 一条 grep 命中。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepMatch {
    pub path: String,
    pub line: i64,
    pub column: i64,
    pub text: String,
}

/// 从构建/测试输出解析出的一条诊断错误，供修复回路使用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagError {
    pub code: String,
    pub message: String,
    pub severity: String,
    pub file: Option<String>,
    pub line: Option<i64>,
    pub column: Option<i64>,
}

// ---------------------------------------------------------------------------
// 旧占位类型：commands / store / agent 等模块仍在引用，
// 待统一迁移到上面的新数据模型（Project / Task）后移除。
// ---------------------------------------------------------------------------

/// 项目唯一标识。TODO: 视需要改为带校验的强类型。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(pub String);

/// 任务唯一标识。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

/// 项目内相对路径。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelPath(pub String);

/// 项目元信息（旧）。TODO: 迁移到 [`Project`]。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub id: ProjectId,
    pub name: String,
    pub root: String,
}

/// 任务元信息（旧）。TODO: 迁移到 [`Task`]。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMeta {
    pub id: TaskId,
    pub project: ProjectId,
    pub title: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn task_status_has_15_camel_case_variants() {
        let cases = [
            (TaskStatus::Idle, "idle"),
            (TaskStatus::Parsing, "parsing"),
            (TaskStatus::Indexing, "indexing"),
            (TaskStatus::Retrieving, "retrieving"),
            (TaskStatus::Planning, "planning"),
            (TaskStatus::Generating, "generating"),
            (TaskStatus::ApplyingPatch, "applyingPatch"),
            (TaskStatus::Verifying, "verifying"),
            (TaskStatus::Repairing, "repairing"),
            (TaskStatus::WaitingConfirm, "waitingConfirm"),
            (TaskStatus::Applied, "applied"),
            (TaskStatus::Done, "done"),
            (TaskStatus::Failed, "failed"),
            (TaskStatus::RolledBack, "rolledBack"),
            (TaskStatus::Cancelled, "cancelled"),
        ];
        assert_eq!(cases.len(), 15);
        for (status, expected) in cases {
            assert_eq!(json!(status), json!(expected));
            let back: TaskStatus = serde_json::from_value(json!(expected)).unwrap();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn project_serializes_with_camel_case_keys() {
        let project = Project {
            id: 1,
            path: "D:/code/demo".to_string(),
            name: "demo".to_string(),
            created_at: 1727568000000,
            last_opened_at: 1727654400000,
        };
        let value = serde_json::to_value(&project).unwrap();
        assert_eq!(value["id"], 1);
        assert_eq!(value["path"], "D:/code/demo");
        assert_eq!(value["name"], "demo");
        assert_eq!(value["createdAt"], 1727568000000i64);
        assert_eq!(value["lastOpenedAt"], 1727654400000i64);
        assert!(value.get("created_at").is_none());
        assert!(value.get("last_opened_at").is_none());
    }

    #[test]
    fn task_optional_fields_serialize_as_null() {
        let task = Task {
            id: 7,
            project_id: 1,
            prompt: "add login".to_string(),
            status: TaskStatus::Planning,
            intent: None,
            plan: Some("1. ...".to_string()),
            created_at: 1727568000000,
            finished_at: None,
            duration_ms: None,
            token_input: Some(120),
            token_output: Some(45),
            cost: Some(0.002),
        };
        let value = serde_json::to_value(&task).unwrap();
        assert_eq!(value["projectId"], 1);
        assert_eq!(value["status"], "planning");
        assert_eq!(value["intent"], Value::Null);
        assert_eq!(value["finishedAt"], Value::Null);
        assert_eq!(value["durationMs"], Value::Null);
        assert_eq!(value["tokenInput"], 120);
        assert_eq!(value["tokenOutput"], 45);
        assert_eq!(value["cost"], 0.002);

        let back: Task = serde_json::from_value(value).unwrap();
        assert_eq!(back, task);
    }

    #[test]
    fn task_deserializes_from_frontend_camel_case_json() {
        let raw = json!({
            "id": 9,
            "projectId": 1,
            "prompt": "重构 types.rs",
            "status": "waitingConfirm",
            "intent": null,
            "plan": "1. 定义结构 2. 测试",
            "createdAt": 1727568000000i64,
            "finishedAt": null,
            "durationMs": null,
            "tokenInput": null,
            "tokenOutput": null,
            "cost": null
        });
        let task: Task = serde_json::from_value(raw).unwrap();
        assert_eq!(task.status, TaskStatus::WaitingConfirm);
        assert_eq!(task.project_id, 1);
        assert_eq!(task.intent, None);
        assert_eq!(task.plan.as_deref(), Some("1. 定义结构 2. 测试"));
    }

    #[test]
    fn task_step_and_patch_roundtrip() {
        let step = TaskStep {
            id: 2,
            task_id: 7,
            step_type: "plan".to_string(),
            content: "分解任务".to_string(),
            created_at: 42,
        };
        let step_back: TaskStep =
            serde_json::from_value(serde_json::to_value(&step).unwrap()).unwrap();
        assert_eq!(step_back, step);

        let patch = Patch {
            id: 3,
            task_id: 7,
            file_path: "src/main.rs".to_string(),
            diff: "--- a\n+++ b".to_string(),
            applied: false,
            score: Some(0.87),
            created_at: 43,
        };
        let value = serde_json::to_value(&patch).unwrap();
        assert_eq!(value["taskId"], 7);
        assert_eq!(value["filePath"], "src/main.rs");
        assert_eq!(value["applied"], false);
        assert_eq!(value["score"], 0.87);
        let patch_back: Patch = serde_json::from_value(value).unwrap();
        assert_eq!(patch_back, patch);
    }

    #[test]
    fn file_record_roundtrip() {
        let record = FileRecord {
            id: 5,
            project_id: 1,
            path: "src/main.rs".to_string(),
            language: Some("rust".to_string()),
            size: Some(1024),
            mtime: Some(1_727_568_000_000),
            hash: Some("blake3:abc".to_string()),
            summary: None,
            indexed_at: Some(1_727_568_000_001),
        };
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["projectId"], 1);
        assert_eq!(value["language"], "rust");
        assert_eq!(value["size"], 1024);
        assert_eq!(value["summary"], Value::Null);
        assert_eq!(value["indexedAt"], 1_727_568_000_001i64);
        let back: FileRecord = serde_json::from_value(value).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn verify_result_and_model_config_roundtrip() {
        let vr = VerifyResult {
            kind: "cargo_test".to_string(),
            passed: true,
            output: "test result: ok".to_string(),
            duration_ms: 1500,
        };
        let value = serde_json::to_value(&vr).unwrap();
        assert_eq!(value["kind"], "cargo_test");
        assert_eq!(value["passed"], true);
        assert_eq!(value["durationMs"], 1500);
        let vr_back: VerifyResult = serde_json::from_value(value).unwrap();
        assert_eq!(vr_back, vr);

        let cfg = ModelConfig {
            provider: "anthropic".to_string(),
            name: "claude-sonnet".to_string(),
            api_key: None,
            base_url: Some("https://api.example.com".to_string()),
            temperature: 0.5,
            max_tokens: Some(8192),
        };
        let value = serde_json::to_value(&cfg).unwrap();
        assert_eq!(value["provider"], "anthropic");
        assert_eq!(value["apiKey"], Value::Null);
        assert_eq!(value["baseUrl"], "https://api.example.com");
        assert_eq!(value["temperature"], 0.5);
        assert_eq!(value["maxTokens"], 8192);
        let cfg_back: ModelConfig = serde_json::from_value(value).unwrap();
        assert_eq!(cfg_back, cfg);
    }

    #[test]
    fn app_config_serializes_model_and_router_with_camel_case() {
        let config = Config {
            model: ModelConfig::default(),
            router: crate::model::router::RouterConfig::default(),
            ..Config::default()
        };
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["model"]["provider"], "deepseek");
        assert_eq!(value["model"]["name"], "deepseek-chat");
        assert_eq!(value["model"]["apiKey"], Value::Null);
        assert_eq!(value["router"]["fastModel"], "deepseek-chat");
        assert_eq!(value["router"]["strongModel"], "deepseek-reasoner");
        assert!(
            value["router"].get("overrides").is_none(),
            "空覆盖不应序列化"
        );

        let back: Config = serde_json::from_value(value).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn file_node_nested_roundtrip() {
        let node = FileNode {
            name: "src".to_string(),
            path: "src".to_string(),
            is_dir: true,
            size: None,
            children: vec![FileNode {
                name: "main.rs".to_string(),
                path: "src/main.rs".to_string(),
                is_dir: false,
                size: Some(1024),
                children: vec![],
            }],
        };
        let value = serde_json::to_value(&node).unwrap();
        assert_eq!(value["isDir"], true);
        assert_eq!(value["size"], Value::Null);
        assert_eq!(value["children"][0]["name"], "main.rs");
        assert_eq!(value["children"][0]["size"], 1024);
        let back: FileNode = serde_json::from_value(value).unwrap();
        assert_eq!(back, node);
    }

    #[test]
    fn context_file_symbol_and_grep_match_roundtrip() {
        let cf = ContextFile {
            path: "src/lib.rs".to_string(),
            content: "fn main() {}".to_string(),
            reason: "入口文件".to_string(),
            tokens: 42,
        };
        let value = serde_json::to_value(&cf).unwrap();
        assert_eq!(value["path"], "src/lib.rs");
        assert_eq!(value["reason"], "入口文件");
        assert_eq!(value["tokens"], 42);

        let sym = Symbol {
            name: "main".to_string(),
            kind: "function".to_string(),
            file: "src/main.rs".to_string(),
            start_line: 1,
            end_line: 3,
        };
        let value = serde_json::to_value(&sym).unwrap();
        assert_eq!(value["file"], "src/main.rs");
        assert_eq!(value["startLine"], 1);
        assert_eq!(value["endLine"], 3);
        let sym_back: Symbol = serde_json::from_value(value).unwrap();
        assert_eq!(sym_back, sym);

        let m = GrepMatch {
            path: "src/main.rs".to_string(),
            line: 10,
            column: 5,
            text: "let x = 1;".to_string(),
        };
        let value = serde_json::to_value(&m).unwrap();
        assert_eq!(value["line"], 10);
        assert_eq!(value["column"], 5);
        let m_back: GrepMatch = serde_json::from_value(value).unwrap();
        assert_eq!(m_back, m);
    }

    #[test]
    fn tool_result_and_diag_error_roundtrip() {
        let tr = ToolResult {
            name: "read_file".to_string(),
            success: true,
            output: "ok".to_string(),
            error: None,
            duration_ms: Some(12),
        };
        let value = serde_json::to_value(&tr).unwrap();
        assert_eq!(value["name"], "read_file");
        assert_eq!(value["success"], true);
        assert_eq!(value["error"], Value::Null);
        assert_eq!(value["durationMs"], 12);
        let tr_back: ToolResult = serde_json::from_value(value).unwrap();
        assert_eq!(tr_back, tr);

        let de = DiagError {
            code: "E0308".to_string(),
            message: "mismatched types".to_string(),
            severity: "error".to_string(),
            file: Some("src/main.rs".to_string()),
            line: Some(10),
            column: None,
        };
        let value = serde_json::to_value(&de).unwrap();
        assert_eq!(value["code"], "E0308");
        assert_eq!(value["file"], "src/main.rs");
        assert_eq!(value["column"], Value::Null);
        let de_back: DiagError = serde_json::from_value(value).unwrap();
        assert_eq!(de_back, de);
    }
}
