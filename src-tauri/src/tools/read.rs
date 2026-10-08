//! 读取文件工具：读取项目内文本文件，支持按行区间截取。
//!
//! 安全策略：
//! - 路径经 canonicalize 解析符号链接与 `..` 后必须仍位于项目根目录内；
//! - 敏感文件（`.env` 系列、`*.key`、`*.pem`）一律拒绝读取；
//! - 存在性检查先于越界检查（canonicalize 需要文件已存在），
//!   不存在的路径只报"文件不存在"，不会泄露项目外的路径信息。
//!
//! TODO: 大文件输出截断；非 UTF-8 编码探测。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::r#trait::{Tool, ToolContext, ToolResult};
use crate::errors::{Result, XaimError};
use crate::index::scanner::is_sensitive;

/// 读取项目文件工具。
pub struct ReadFileTool;

/// 读取可选的整数行号参数；`null` 视为缺省，类型不对按调用链错误处理。
fn line_arg(args: &Value, key: &str) -> Result<Option<i64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| XaimError::Unknown(format!("参数 {key} 必须是整数"))),
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "读取项目内文本文件的内容，可按 1 起始的行区间截取"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "项目内相对路径或绝对路径" },
                "start": { "type": "integer", "description": "起始行号（1 起始，含），缺省从第 1 行" },
                "end": { "type": "integer", "description": "结束行号（含），缺省到文件末尾" }
            },
            "required": ["path"]
        })
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let raw = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| XaimError::Unknown("缺少 path 参数".into()))?;
        if raw.trim().is_empty() {
            return Err(XaimError::Unknown("path 不能为空".into()));
        }
        let start = line_arg(&args, "start")?;
        let end = line_arg(&args, "end")?;
        for (key, value) in [("start", start), ("end", end)] {
            if value.is_some_and(|n| n < 1) {
                return Err(XaimError::Unknown(format!("{key} 必须是不小于 1 的行号")));
            }
        }
        if let (Some(s), Some(e)) = (start, end) {
            if s > e {
                return Err(XaimError::Unknown(format!(
                    "start ({s}) 不能大于 end ({e})"
                )));
            }
        }

        let target = Path::new(raw);
        let candidate: PathBuf = if target.is_absolute() {
            target.to_path_buf()
        } else {
            ctx.project_root.join(target)
        };

        // 存在性与目录检查：canonicalize 只对已存在的路径生效。
        let meta = match tokio::fs::metadata(&candidate).await {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ToolResult::fail(format!(
                    "文件不存在: {}",
                    candidate.display()
                )));
            }
            Err(e) => return Err(e.into()),
        };
        if meta.is_dir() {
            return Ok(ToolResult::fail(format!(
                "路径指向目录而非文件: {}",
                candidate.display()
            )));
        }

        // 越界检查：解析符号链接与 `..` 后必须仍在项目根目录内。
        let canonical_root = tokio::fs::canonicalize(&ctx.project_root).await?;
        let resolved = tokio::fs::canonicalize(&candidate).await?;
        if !resolved.starts_with(&canonical_root) {
            return Err(XaimError::Security(format!("路径越出项目目录: {raw}")));
        }
        if let Some(name) = resolved.file_name().and_then(|name| name.to_str()) {
            if is_sensitive(name) {
                return Err(XaimError::Security(format!("拒绝读取敏感文件: {name}")));
            }
        }

        let content = match tokio::fs::read_to_string(&resolved).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                return Ok(ToolResult::fail(format!(
                    "文件不是有效的 UTF-8 文本: {}",
                    resolved.display()
                )));
            }
            Err(e) => return Err(e.into()),
        };

        // 行区间截取（1 起始、双端包含）；未给区间时返回全文，end 超出文件末尾时截断。
        let output = if start.is_none() && end.is_none() {
            content
        } else {
            let total = content.lines().count();
            let s = start.unwrap_or(1) as usize;
            let e = end.unwrap_or(total as i64).min(total as i64) as usize;
            if s > total {
                return Ok(ToolResult::fail(format!(
                    "start 超出文件行数: 文件共 {total} 行，请求从第 {s} 行开始"
                )));
            }
            content
                .lines()
                .skip(s - 1)
                .take(e - s + 1)
                .collect::<Vec<_>>()
                .join("\n")
        };

        Ok(ToolResult::ok(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::store::Store;

    const FIVE_LINES: &str = "l1\nl2\nl3\nl4\nl5";

    fn ctx_with_root(root: &Path) -> ToolContext {
        ToolContext {
            project_root: root.to_path_buf(),
            store: Arc::new(Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        }
    }

    async fn read_file(
        ctx: &ToolContext,
        args: Value,
    ) -> std::result::Result<ToolResult, XaimError> {
        ReadFileTool.call(args, ctx).await
    }

    #[tokio::test]
    async fn reads_whole_file_without_range() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), FIVE_LINES).unwrap();
        let ctx = ctx_with_root(dir.path());

        let result = read_file(&ctx, json!({ "path": "note.txt" }))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, FIVE_LINES);
    }

    #[tokio::test]
    async fn reads_inclusive_line_range() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), FIVE_LINES).unwrap();
        let ctx = ctx_with_root(dir.path());

        let result = read_file(&ctx, json!({ "path": "note.txt", "start": 2, "end": 4 }))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "l2\nl3\nl4");
    }

    #[tokio::test]
    async fn open_ended_ranges_default_to_file_bounds() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), FIVE_LINES).unwrap();
        let ctx = ctx_with_root(dir.path());

        let tail = read_file(&ctx, json!({ "path": "note.txt", "start": 4 }))
            .await
            .unwrap();
        assert_eq!(tail.output, "l4\nl5");

        let head = read_file(&ctx, json!({ "path": "note.txt", "end": 2 }))
            .await
            .unwrap();
        assert_eq!(head.output, "l1\nl2");
    }

    #[tokio::test]
    async fn end_beyond_eof_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), FIVE_LINES).unwrap();
        let ctx = ctx_with_root(dir.path());

        let result = read_file(&ctx, json!({ "path": "note.txt", "start": 3, "end": 99 }))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "l3\nl4\nl5");
    }

    #[tokio::test]
    async fn start_beyond_eof_fails_with_total_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), FIVE_LINES).unwrap();
        let ctx = ctx_with_root(dir.path());

        let result = read_file(&ctx, json!({ "path": "note.txt", "start": 10 }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("共 5 行"));
    }

    #[tokio::test]
    async fn empty_file_reads_as_empty_output() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();
        let ctx = ctx_with_root(dir.path());

        let result = read_file(&ctx, json!({ "path": "empty.txt" }))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "");
    }

    #[tokio::test]
    async fn rejects_env_key_and_pem_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in [".env", ".env.local", "server.KEY", "id_rsa.pem"] {
            std::fs::write(dir.path().join(name), "secret").unwrap();
        }
        let ctx = ctx_with_root(dir.path());

        for name in [".env", ".env.local", "server.KEY", "id_rsa.pem"] {
            let err = read_file(&ctx, json!({ "path": name })).await.unwrap_err();
            assert!(
                matches!(err, XaimError::Security(_)),
                "{name} 应被安全策略拒绝"
            );
        }
    }

    #[tokio::test]
    async fn allows_similar_but_non_sensitive_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.envx"), "ok").unwrap();
        std::fs::write(dir.path().join("env"), "ok").unwrap();
        let ctx = ctx_with_root(dir.path());

        for name in ["notes.envx", "env"] {
            let result = read_file(&ctx, json!({ "path": name })).await.unwrap();
            assert!(result.success, "{name} 不在敏感名单内，应可读取");
        }
    }

    #[tokio::test]
    async fn rejects_escape_via_parent_traversal() {
        // 两个兄弟临时目录：project/ 与 outside/，用 `../outside/x.txt` 越界。
        let parent = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir_in(parent.path()).unwrap();
        let outside = tempfile::tempdir_in(parent.path()).unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top secret").unwrap();
        let ctx = ctx_with_root(project.path());

        let rel = format!(
            "../{}/secret.txt",
            outside.path().file_name().unwrap().to_str().unwrap()
        );
        let err = read_file(&ctx, json!({ "path": rel })).await.unwrap_err();
        assert!(matches!(err, XaimError::Security(_)));
    }

    #[tokio::test]
    async fn rejects_absolute_path_outside_project() {
        let parent = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir_in(parent.path()).unwrap();
        let outside = tempfile::tempdir_in(parent.path()).unwrap();
        let target = outside.path().join("secret.txt");
        std::fs::write(&target, "top secret").unwrap();
        let ctx = ctx_with_root(project.path());

        let err = read_file(&ctx, json!({ "path": target.display().to_string() }))
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Security(_)));
    }

    #[tokio::test]
    async fn missing_file_fails_with_clear_message() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with_root(dir.path());

        let result = read_file(&ctx, json!({ "path": "ghost.txt" }))
            .await
            .unwrap();
        assert!(!result.success, "文件不存在应走 Ok(ToolResult) 通道");
        assert!(result.error.unwrap().contains("文件不存在"));
    }

    #[tokio::test]
    async fn directory_path_fails_with_clear_message() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let ctx = ctx_with_root(dir.path());

        let result = read_file(&ctx, json!({ "path": "sub" })).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("目录"));
    }

    #[tokio::test]
    async fn missing_path_arg_is_call_chain_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with_root(dir.path());

        let err = read_file(&ctx, json!({})).await.unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));
    }

    #[tokio::test]
    async fn non_integer_line_arg_is_call_chain_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), FIVE_LINES).unwrap();
        let ctx = ctx_with_root(dir.path());

        let err = read_file(&ctx, json!({ "path": "note.txt", "start": "2" }))
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));
    }

    #[tokio::test]
    async fn start_greater_than_end_is_call_chain_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note.txt"), FIVE_LINES).unwrap();
        let ctx = ctx_with_root(dir.path());

        let err = read_file(&ctx, json!({ "path": "note.txt", "start": 3, "end": 2 }))
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));
    }

    #[tokio::test]
    async fn non_utf8_file_fails_with_clear_message() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blob.bin"), [0xffu8, 0xfe]).unwrap();
        let ctx = ctx_with_root(dir.path());

        let result = read_file(&ctx, json!({ "path": "blob.bin" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("UTF-8"));
    }
}
