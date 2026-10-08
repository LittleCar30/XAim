//! 工具注册表：启动期集中注册全部内置工具，运行期按名称分发调用。
//!
//! 线程安全约定：注册表自身不加锁——注册只发生在启动期（`&mut self` 独占借用），
//! 之后以 `Arc<ToolRegistry>` 在多任务间共享，查询与调用只取 `&self`；
//! `Tool: Send + Sync` 约束保证 `Box<dyn Tool>` 可跨线程传递（测试中有静态断言）。

use std::collections::HashMap;

use serde_json::Value;

use super::r#trait::{Tool, ToolContext, ToolResult};
use crate::errors::{Result, XaimError};

/// 按名称索引的工具注册表。
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    /// 创建空注册表。
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// 注册一个工具；同名重复注册时覆盖旧实现并记录警告（注册期是配置问题，
    /// 不应 panic，且保留后注册的实现便于测试中替换桩）。
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        if self.tools.insert(name.clone(), tool).is_some() {
            tracing::warn!(tool = %name, "工具重复注册，旧实现已被覆盖");
        }
    }

    /// 按名称查找工具。
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|tool| tool.as_ref())
    }

    /// 全部已注册工具的名称，按字典序排序，保证给模型的工具清单顺序稳定。
    pub fn list(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.tools.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// 按名称分发一次工具调用；未注册的名称返回 `XaimError::Unknown`。
    pub async fn call(&self, name: &str, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| XaimError::Unknown(format!("未注册的工具: {name}")))?;
        tool.call(args, ctx).await
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::store::Store;

    struct EchoTool;

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "原样回显 text 参数"
        }

        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }

        async fn call(&self, args: Value, _ctx: &ToolContext) -> Result<ToolResult> {
            Ok(ToolResult::ok(
                args.get("text").and_then(Value::as_str).unwrap_or_default(),
            ))
        }
    }

    struct BoomTool;

    #[async_trait::async_trait]
    impl Tool for BoomTool {
        fn name(&self) -> &str {
            "boom"
        }

        fn description(&self) -> &str {
            "总是返回执行失败"
        }

        fn schema(&self) -> Value {
            json!({ "type": "object" })
        }

        async fn call(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolResult> {
            Ok(ToolResult::fail("炸了"))
        }
    }

    struct EchoV2Tool;

    #[async_trait::async_trait]
    impl Tool for EchoV2Tool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "第二版回显"
        }

        fn schema(&self) -> Value {
            json!({ "type": "object" })
        }

        async fn call(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolResult> {
            Ok(ToolResult::ok("v2"))
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

    #[test]
    fn register_and_get_round_trip() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let tool = registry.get("echo").expect("注册后应能按名称查到");
        assert_eq!(tool.name(), "echo");
        assert_eq!(tool.description(), "原样回显 text 参数");
        assert!(registry.get("nope").is_none(), "未注册的名称应返回 None");
    }

    #[test]
    fn duplicate_register_replaces_old_implementation() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(EchoV2Tool));

        assert_eq!(registry.list(), ["echo"], "同名注册应覆盖而非新增条目");
        assert_eq!(
            registry.get("echo").unwrap().description(),
            "第二版回显",
            "被覆盖后应取到新实现"
        );
    }

    #[test]
    fn list_returns_sorted_names() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(BoomTool));
        registry.register(Box::new(EchoV2Tool));

        assert_eq!(registry.list(), ["boom", "echo"]);
    }

    #[tokio::test]
    async fn call_dispatches_args_to_named_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let ctx = test_ctx();
        let result = registry
            .call("echo", json!({ "text": "hi" }), &ctx)
            .await
            .unwrap();
        assert_eq!(result.output, "hi");
        assert!(result.success);
    }

    #[tokio::test]
    async fn call_propagates_tool_failure_result() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(BoomTool));

        let ctx = test_ctx();
        let result = registry.call("boom", json!({}), &ctx).await.unwrap();
        assert!(
            !result.success,
            "工具执行失败应走 Ok(ToolResult) 通道而非 Err"
        );
        assert_eq!(result.error.as_deref(), Some("炸了"));
    }

    #[tokio::test]
    async fn call_unknown_tool_is_unknown_error() {
        let registry = ToolRegistry::new();
        let ctx = test_ctx();
        let err = registry.call("ghost", json!({}), &ctx).await.unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));
    }

    #[test]
    fn registry_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ToolRegistry>();
        assert_send_sync::<ToolContext>();
    }
}
