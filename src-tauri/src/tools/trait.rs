//! 工具抽象：模型可调用能力的统一 trait、执行上下文与结果模型。
//!
//! `trait` 是 Rust 关键字，本模块经 `pub mod r#trait` 引入，
//! 使用方写作 `use super::r#trait::Tool`（或完整路径 `tools::r#trait::Tool`）。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

use crate::errors::Result;
use crate::store::Store;

/// 工具执行上下文：一次任务期间共享给所有工具调用的环境信息。
#[derive(Clone)]
pub struct ToolContext {
    /// 当前项目根目录，工具收到的相对路径都应解析到该目录之内。
    pub project_root: PathBuf,
    /// 全局 SQLite 存储入口（`Arc` 共享，连接内部有互斥锁）。
    pub store: Arc<Store>,
    /// 为 true 时危险操作需先经用户确认；解释权在编排器与具体工具。
    pub require_confirm: bool,
    /// exec 类工具在内置默认白名单之外额外放行的命令；空表表示仅用默认白名单。
    pub allowed_commands: Vec<String>,
}

/// 一次工具调用的执行结果（内部模型；传给前端的 DTO 见 `types::ToolResult`）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolResult {
    /// 是否成功。失败时 `error` 必有内容，`output` 可能为空。
    pub success: bool,
    /// 成功时给模型看的结果文本（命令输出、文件内容等）。
    pub output: String,
    /// 失败原因。
    pub error: Option<String>,
}

impl ToolResult {
    /// 构造成功结果。
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
        }
    }

    /// 构造失败结果。
    pub fn fail(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error.into()),
        }
    }
}

/// 所有内置工具实现的统一抽象。
///
/// `call` 的两条失败通道约定：
/// - 返回 `Err(XaimError)` —— 调用链本身失败（参数非法、存储错误等），由编排器按异常处理；
/// - 返回 `Ok(ToolResult { success: false, .. })` —— 工具执行了但结果为失败
///   （编译不通过、文件不存在等），结果文本要回灌给模型继续推理。
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具唯一名称（小写下划线），供模型调用与注册表查找。
    fn name(&self) -> &str;

    /// 一句话描述，进入给模型的工具清单提示词。
    fn description(&self) -> &str;

    /// 参数的 JSON Schema（`{"type": "object", "properties": {...}}` 形式），
    /// 原样进入模型请求的 tools 字段。
    fn schema(&self) -> Value;

    /// 执行工具。`args` 为模型生成的 JSON 参数原文。
    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::XaimError;
    use serde_json::json;

    struct UpperTool;

    #[async_trait]
    impl Tool for UpperTool {
        fn name(&self) -> &str {
            "upper"
        }

        fn description(&self) -> &str {
            "把文本转成大写"
        }

        fn schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            })
        }

        async fn call(&self, args: Value, _ctx: &ToolContext) -> Result<ToolResult> {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| XaimError::Unknown("缺少 text 参数".into()))?;
            Ok(ToolResult::ok(text.to_uppercase()))
        }
    }

    fn test_ctx() -> ToolContext {
        ToolContext {
            project_root: PathBuf::from("."),
            store: Arc::new(Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        }
    }

    #[tokio::test]
    async fn call_reads_args_from_json() {
        let ctx = test_ctx();
        let result = UpperTool
            .call(json!({ "text": "hello" }), &ctx)
            .await
            .unwrap();
        assert_eq!(result.output, "HELLO");
        assert!(result.success);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn call_missing_arg_is_err() {
        let ctx = test_ctx();
        let err = UpperTool.call(json!({}), &ctx).await.unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));
    }

    #[test]
    fn schema_is_a_json_schema_object() {
        let schema = UpperTool.schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].is_object());
        assert_eq!(schema["required"][0], "text");
    }

    #[test]
    fn tool_result_constructors_cover_both_channels() {
        let ok = ToolResult::ok("done");
        assert_eq!(
            ok,
            ToolResult {
                success: true,
                output: "done".into(),
                error: None
            }
        );

        let fail = ToolResult::fail("boom");
        assert!(!fail.success);
        assert_eq!(fail.error.as_deref(), Some("boom"));
    }

    #[test]
    fn context_is_shareable_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ToolContext>();
    }
}
