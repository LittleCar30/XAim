//! 工具层：模型可调用的原子能力（trait 为 Rust 关键字，模块名用 r#trait）。

pub mod exec;
pub mod patch;
pub mod read;
pub mod registry;
pub mod search;
pub mod r#trait;
pub mod write;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::r#trait::ToolContext;
    use super::registry::ToolRegistry;
    use crate::store::Store;

    fn builtins() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(super::read::ReadFileTool));
        registry.register(Box::new(super::write::WriteFileTool));
        registry.register(Box::new(super::search::SearchTool));
        registry.register(Box::new(super::exec::RunCommandTool));
        registry
    }

    #[tokio::test]
    async fn registered_tools_share_workspace_through_write_read_search_patch_and_exec() {
        let dir = tempfile::tempdir().unwrap();
        let context = ToolContext {
            project_root: dir.path().to_path_buf(),
            store: Arc::new(Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        };
        let registry = builtins();
        let old = "pub fn login() {}\n";
        let new = "pub fn login() { println!(\"ok\"); }\n";
        let result = registry
            .call(
                "write_file",
                json!({"path": "src/auth.rs", "content": old}),
                &context,
            )
            .await
            .unwrap();
        assert!(result.success, "{result:?}");
        let result = registry
            .call("read_file", json!({"path": "src/auth.rs"}), &context)
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, old);
        let result = registry
            .call("search", json!({"query": "login"}), &context)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("src/auth.rs:1:"));

        let diff = super::patch::generate_diff(old, new, "src/auth.rs");
        super::patch::validate_diff(&diff, dir.path()).unwrap();
        super::patch::apply_diff(&diff, dir.path()).unwrap();
        let result = registry
            .call("read_file", json!({"path": "src/auth.rs"}), &context)
            .await
            .unwrap();
        assert_eq!(result.output, new);
        super::patch::reverse_diff(&diff, dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/auth.rs")).unwrap(),
            old
        );

        let result = registry
            .call(
                "run_command",
                json!({"command": "cargo --version"}),
                &context,
            )
            .await
            .unwrap();
        assert!(result.success, "{result:?}");
        assert!(result.output.starts_with("cargo "));
    }

    #[tokio::test]
    async fn every_builtin_rejects_non_object_arguments_without_workspace_changes() {
        let dir = tempfile::tempdir().unwrap();
        let context = ToolContext {
            project_root: dir.path().to_path_buf(),
            store: Arc::new(Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        };
        let registry = builtins();
        for name in registry.list() {
            for args in [json!(null), json!([]), json!("invalid"), json!(42)] {
                assert!(registry.call(name, args, &context).await.is_err(), "{name}");
            }
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
