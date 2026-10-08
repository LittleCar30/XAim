//! 检索工具：把 `search::grep` 的内容检索包装成模型可调用的 search 工具。
//!
//! 检索范围与文件树一致：遵守 `.gitignore` 与默认忽略表，`.env`、`*.key`
//! 等敏感文件不会出现在结果里（由 [`crate::search::grep`] 层保证）。
//!
//! TODO: 聚合符号查找入口，按需暴露 `regex`/`file_types` 等高级参数。

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::r#trait::{Tool, ToolContext, ToolResult};
use crate::errors::{Result, XaimError};
use crate::search::grep::{self, GrepMatch, GrepOptions};

/// 单次检索允许的结果条数上限，防止模型请求超大上限触发全量遍历收集。
const MAX_RESULTS_CAP: usize = 1000;

/// 项目内全文检索工具。
pub struct SearchTool;

/// 解析可选的 `max_results`：缺省用 grep 层默认值（200），
/// 非法值按调用链错误处理，超出 [`MAX_RESULTS_CAP`] 时静默截断。
fn parse_max_results(args: &Value) -> Result<usize> {
    let Some(v) = args.get("max_results").filter(|v| !v.is_null()) else {
        return Ok(GrepOptions::default().max_results);
    };
    let n = v
        .as_i64()
        .ok_or_else(|| XaimError::Unknown("参数 max_results 必须是整数".into()))?;
    if n < 1 {
        return Err(XaimError::Unknown("max_results 必须是正整数".into()));
    }
    Ok(n.min(MAX_RESULTS_CAP as i64) as usize)
}

/// 把命中清单格式化成 `path:line:col: text` 逐行文本，末尾附命中数摘要；
/// 达到上限时提示模型可能还有更多命中。
fn format_output(query: &str, matches: &[GrepMatch], max_results: usize) -> String {
    if matches.is_empty() {
        return format!("未找到匹配: {query}");
    }
    let mut lines: Vec<String> = matches
        .iter()
        .map(|m| format!("{}:{}:{}: {}", m.path, m.line, m.column, m.text))
        .collect();
    let mut summary = format!("共 {} 条命中", matches.len());
    if matches.len() >= max_results {
        summary.push_str(&format!("（已达上限 {max_results}，可能还有更多）"));
    }
    lines.push(summary);
    lines.join("\n")
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "在项目内全文检索代码与文本（默认按字面量、大小写不敏感），返回命中行清单"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "要检索的内容（默认字面量匹配，正则元字符不生效）" },
                "max_results": { "type": "integer", "description": "结果条数上限，默认 200，最大 1000" }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| XaimError::Unknown("缺少 query 参数".into()))?;
        if query.trim().is_empty() {
            return Err(XaimError::Unknown("query 不能为空".into()));
        }
        let max_results = parse_max_results(&args)?;
        let opts = GrepOptions {
            max_results,
            ..GrepOptions::default()
        };

        // grep 遍历是同步阻塞操作，放到阻塞线程池执行，避免占住 tokio worker。
        let root = PathBuf::from(&ctx.project_root);
        let pattern = query.to_string();
        let matches = tokio::task::spawn_blocking(move || grep::search(&root, &pattern, opts))
            .await
            .map_err(|e| XaimError::Unknown(format!("检索任务异常退出: {e}")))??;

        Ok(ToolResult::ok(format_output(query, &matches, max_results)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use crate::store::Store;

    fn ctx_with_root(root: &Path) -> ToolContext {
        ToolContext {
            project_root: root.to_path_buf(),
            store: Arc::new(Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        }
    }

    /// 在 `root` 下创建相对路径 `rel` 的文件（自动补建父目录）。
    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    async fn search(ctx: &ToolContext, args: Value) -> std::result::Result<ToolResult, XaimError> {
        SearchTool.call(args, ctx).await
    }

    #[tokio::test]
    async fn finds_matches_across_files() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "src/a.txt", "needle one\nplain\nneedle two\n");
        write_file(dir.path(), "docs/b.md", "needle here\n");
        let ctx = ctx_with_root(dir.path());

        let result = search(&ctx, json!({ "query": "needle" })).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("src/a.txt:1:1: needle one"));
        assert!(result.output.contains("src/a.txt:3:1: needle two"));
        assert!(result.output.contains("docs/b.md:1:1: needle here"));
        assert!(result.output.contains("共 3 条命中"));
    }

    #[tokio::test]
    async fn max_results_caps_output_with_notice() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "many.txt", "needle\n".repeat(10).as_str());
        let ctx = ctx_with_root(dir.path());

        let result = search(&ctx, json!({ "query": "needle", "max_results": 3 }))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("many.txt:1:"), "{}", result.output);
        assert!(result.output.contains("many.txt:3:"));
        assert!(!result.output.contains("many.txt:4:"), "应在 3 条处截断");
        assert!(result.output.contains("已达上限 3"));
    }

    #[tokio::test]
    async fn no_match_is_success_with_notice() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.txt", "hello\n");
        let ctx = ctx_with_root(dir.path());

        let result = search(&ctx, json!({ "query": "ghost" })).await.unwrap();
        assert!(result.success, "检索无命中属正常结果而非失败");
        assert!(result.output.contains("未找到匹配"));
    }

    #[tokio::test]
    async fn default_query_is_literal_and_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "case.txt", "Hello World\n");
        write_file(dir.path(), "lit.txt", "call f.n now\n");
        write_file(dir.path(), "fun.txt", "call fun now\n");
        let ctx = ctx_with_root(dir.path());

        let result = search(&ctx, json!({ "query": "hello world" }))
            .await
            .unwrap();
        assert!(result.output.contains("case.txt:1:1: Hello World"));

        let result = search(&ctx, json!({ "query": "f.n" })).await.unwrap();
        assert!(
            result.output.contains("lit.txt:1:"),
            "字面量模式下 . 不应通配"
        );
        assert!(!result.output.contains("fun.txt"));
    }

    #[tokio::test]
    async fn sensitive_and_ignored_files_never_appear() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), ".env", "SECRET=needle\n");
        write_file(dir.path(), "server.key", "needle\n");
        write_file(dir.path(), "node_modules/pkg.js", "needle\n");
        write_file(dir.path(), "src/main.rs", "needle\n");
        let ctx = ctx_with_root(dir.path());

        let result = search(&ctx, json!({ "query": "needle" })).await.unwrap();
        assert!(result.output.contains("src/main.rs"));
        assert!(!result.output.contains(".env"), "敏感文件不应出现在结果中");
        assert!(!result.output.contains("server.key"));
        assert!(!result.output.contains("node_modules"));
    }

    #[tokio::test]
    async fn missing_or_blank_query_is_call_chain_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with_root(dir.path());

        let err = search(&ctx, json!({})).await.unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));

        let err = search(&ctx, json!({ "query": "   " })).await.unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));
    }

    #[tokio::test]
    async fn invalid_max_results_is_call_chain_error() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.txt", "needle\n");
        let ctx = ctx_with_root(dir.path());

        for bad in [0, -5] {
            let err = search(&ctx, json!({ "query": "needle", "max_results": bad }))
                .await
                .unwrap_err();
            assert!(matches!(err, XaimError::Unknown(_)), "{bad} 应报调用链错误");
        }
        let err = search(&ctx, json!({ "query": "needle", "max_results": "10" }))
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Unknown(_)));
    }

    #[test]
    fn max_results_defaults_and_clamps() {
        assert_eq!(parse_max_results(&json!({})).unwrap(), 200);
        assert_eq!(
            parse_max_results(&json!({ "max_results": null })).unwrap(),
            200
        );
        assert_eq!(parse_max_results(&json!({ "max_results": 5 })).unwrap(), 5);
        assert_eq!(
            parse_max_results(&json!({ "max_results": 100_000 })).unwrap(),
            MAX_RESULTS_CAP,
            "超大上限应被截断"
        );
    }
}
