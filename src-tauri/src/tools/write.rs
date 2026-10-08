//! 写入文件工具：创建或覆盖项目内文本文件（高风险，默认需用户确认）。
//!
//! 安全策略（依序执行）：
//! 1. 目标路径词法归一化（解析 `.`/`..`）后，对最深已存在祖先 canonicalize
//!    （解析符号链接），再拼回尚未创建的后缀组件，校验必须仍位于项目根
//!    目录内——新文件的父目录可能尚不存在，不能直接 canonicalize 目标本身；
//! 2. 敏感文件（`.env` 系列、`*.key`、`*.pem`，复用 [`crate::index::scanner`]）
//!    一律拒绝写入；
//! 3. `ctx.require_confirm` 为 true 时不做任何文件系统改动（含创建父目录），
//!    返回以 `需确认：` 开头的失败结果——该前缀是编排器的识别约定，
//!    征得用户同意后以 `require_confirm = false` 重新调用即可落盘。
//!
//! TODO: 落盘前快照（接入 store::patch）；换行符处理。

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::r#trait::{Tool, ToolContext, ToolResult};
use crate::errors::{Result, XaimError};
use crate::index::scanner::is_sensitive;

/// 创建或覆盖项目文件工具。
pub struct WriteFileTool;

/// 词法归一化：解析 `.` 与 `..`，不触碰文件系统。绝对路径上 `..` 越过根时
/// 钳制在根（随后由项目内前缀检查拒绝）。
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // 只有普通组件可弹出；前缀/根组件保留（`C:\..` 归一为 `C:\`）。
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                }
            }
            component => normalized.push(component),
        }
    }
    normalized
}

/// 把候选路径解析为可写入的真实路径：对最深已存在祖先 canonicalize，
/// 再按序拼回尚不存在的后缀组件。已存在的中间目录若是符号链接会被
/// 解析，指向项目外的链接因此在越界检查中被拦截。
async fn resolve_target(candidate: &Path) -> Result<PathBuf> {
    let normalized = normalize(candidate);
    let mut prefix = normalized.clone();
    let mut missing: Vec<OsString> = Vec::new();
    let existing = loop {
        match tokio::fs::canonicalize(&prefix).await {
            Ok(existing) => break existing,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let last = match prefix.file_name() {
                    Some(name) => name.to_os_string(),
                    None => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "路径根目录不存在",
                        )
                        .into())
                    }
                };
                prefix.pop();
                missing.push(last);
            }
            Err(e) => return Err(e.into()),
        }
    };
    let mut resolved = existing;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

/// 读取必需的字符串参数，缺参与类型错都按调用链错误处理。
fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(XaimError::Unknown(format!("{key} 必须是字符串"))),
        None => Err(XaimError::Unknown(format!("缺少 {key} 参数"))),
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "创建或覆盖项目内文本文件（高风险，需用户确认后执行）"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "项目内相对路径（父目录不存在时自动创建）或绝对路径" },
                "content": { "type": "string", "description": "要写入的完整文件内容（覆盖式写入）" }
            },
            "required": ["path", "content"]
        })
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let raw = string_arg(&args, "path")?;
        if raw.trim().is_empty() {
            return Err(XaimError::Unknown("path 不能为空".into()));
        }
        let content = string_arg(&args, "content")?;

        // 路径校验：归一化 + 符号链接解析后必须仍在项目根目录内。
        let target = Path::new(raw);
        let candidate = if target.is_absolute() {
            target.to_path_buf()
        } else {
            ctx.project_root.join(target)
        };
        let canonical_root = tokio::fs::canonicalize(&ctx.project_root).await?;
        let resolved = resolve_target(&candidate).await?;
        if !resolved.starts_with(&canonical_root) {
            return Err(XaimError::Security(format!("路径越出项目目录: {raw}")));
        }

        // 敏感文件拒绝。
        if let Some(name) = resolved.file_name().and_then(|name| name.to_str()) {
            if is_sensitive(name) {
                return Err(XaimError::Security(format!("拒绝写入敏感文件: {name}")));
            }
        }

        let existed = match tokio::fs::metadata(&resolved).await {
            Ok(meta) if meta.is_dir() => {
                return Ok(ToolResult::fail(format!("路径指向目录而非文件: {raw}")));
            }
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };

        // 确认门：未确认前不做任何文件系统改动。
        if ctx.require_confirm {
            return Ok(ToolResult::fail(format!(
                "需确认：即将{} {raw}，须征得用户同意后执行",
                if existed {
                    "覆盖文件"
                } else {
                    "创建文件"
                }
            )));
        }

        // 落盘：父目录不存在则自动创建。
        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&resolved, content).await?;

        let action = if existed { "已覆盖" } else { "已创建" };
        Ok(ToolResult::ok(format!(
            "{action} {raw}（{} 字节）",
            content.len()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;

    use crate::store::Store;

    fn ctx_with(root: &Path, require_confirm: bool) -> ToolContext {
        ToolContext {
            project_root: root.to_path_buf(),
            store: Arc::new(Store::open_in_memory().unwrap()),
            require_confirm,
            allowed_commands: vec![],
        }
    }

    async fn write_file(
        ctx: &ToolContext,
        args: Value,
    ) -> std::result::Result<ToolResult, XaimError> {
        WriteFileTool.call(args, ctx).await
    }

    #[tokio::test]
    async fn creates_new_file_including_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with(dir.path(), false);

        let result = write_file(
            &ctx,
            json!({ "path": "src/new/deep/main.rs", "content": "fn main() {}\n" }),
        )
        .await
        .unwrap();

        assert!(result.success);
        assert!(result.output.contains("已创建"));
        assert!(result.output.contains("字节"));
        let written = fs::read_to_string(dir.path().join("src/new/deep/main.rs")).unwrap();
        assert_eq!(written, "fn main() {}\n");
    }

    #[tokio::test]
    async fn overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.txt"), "old\n").unwrap();
        let ctx = ctx_with(dir.path(), false);

        let result = write_file(
            &ctx,
            json!({ "path": "note.txt", "content": "new content\n" }),
        )
        .await
        .unwrap();

        assert!(result.success);
        assert!(result.output.contains("已覆盖"));
        let written = fs::read_to_string(dir.path().join("note.txt")).unwrap();
        assert_eq!(written, "new content\n");
    }

    #[tokio::test]
    async fn confirm_gate_blocks_any_filesystem_change_until_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let args = json!({ "path": "out/new.txt", "content": "x" });

        let blocked = write_file(&ctx_with(dir.path(), true), args.clone())
            .await
            .unwrap();
        assert!(!blocked.success, "未确认的写入应以失败结果返回");
        assert!(blocked.error.unwrap().starts_with("需确认"));
        assert!(
            !dir.path().join("out").exists(),
            "确认门前不应创建父目录等任何文件系统改动"
        );

        let released = write_file(&ctx_with(dir.path(), false), args)
            .await
            .unwrap();
        assert!(released.success, "确认后同一调用应能落盘");
        assert_eq!(
            fs::read_to_string(dir.path().join("out/new.txt")).unwrap(),
            "x"
        );
    }

    #[tokio::test]
    async fn rejects_sensitive_files() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with(dir.path(), false);

        for name in [".env", ".env.local", "id_rsa.KEY", "cert.pem"] {
            let err = write_file(&ctx, json!({ "path": name, "content": "secret" }))
                .await
                .unwrap_err();
            assert!(matches!(err, XaimError::Security(_)), "{name} 应被拒绝");
            assert!(!dir.path().join(name).exists(), "{name} 不应被落盘");
        }
    }

    #[tokio::test]
    async fn rejects_escape_via_parent_traversal() {
        let parent = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir_in(parent.path()).unwrap();
        let outside = tempfile::tempdir_in(parent.path()).unwrap();
        let ctx = ctx_with(project.path(), false);

        let rel = format!(
            "../{}/x.txt",
            outside.path().file_name().unwrap().to_str().unwrap()
        );
        let err = write_file(&ctx, json!({ "path": rel, "content": "evil" }))
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Security(_)));
        assert!(
            !outside.path().join("x.txt").exists(),
            "越界路径不应在项目外落盘"
        );
    }

    #[tokio::test]
    async fn rejects_absolute_path_outside_project() {
        let parent = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir_in(parent.path()).unwrap();
        let outside = tempfile::tempdir_in(parent.path()).unwrap();
        let target = outside.path().join("x.txt");
        let ctx = ctx_with(project.path(), false);

        let err = write_file(
            &ctx,
            json!({ "path": target.display().to_string(), "content": "evil" }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, XaimError::Security(_)));
        assert!(!target.exists(), "越界绝对路径不应落盘");
    }

    #[tokio::test]
    async fn rejects_writing_over_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        let ctx = ctx_with(dir.path(), false);

        let result = write_file(&ctx, json!({ "path": "sub", "content": "x" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("目录"));
    }

    #[tokio::test]
    async fn invalid_args_are_call_chain_errors() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with(dir.path(), false);

        for bad in [
            json!({}),
            json!({ "path": "a.txt" }),
            json!({ "path": "a.txt", "content": 123 }),
            json!({ "path": 1, "content": "x" }),
            json!({ "path": "   ", "content": "x" }),
        ] {
            let err = write_file(&ctx, bad.clone()).await.unwrap_err();
            assert!(matches!(err, XaimError::Unknown(_)), "{bad} 应报调用链错误");
        }
    }

    #[tokio::test]
    async fn empty_content_creates_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with(dir.path(), false);

        let result = write_file(&ctx, json!({ "path": "empty.txt", "content": "" }))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(fs::metadata(dir.path().join("empty.txt")).unwrap().len(), 0);
    }

    #[test]
    fn normalize_resolves_dot_segments_lexically() {
        assert_eq!(
            normalize(Path::new("/p/./a/../b")),
            normalize(Path::new("/p/b"))
        );
        assert_eq!(
            normalize(Path::new("/p/a/../../b")),
            normalize(Path::new("/p/../b"))
        );
        assert_eq!(normalize(Path::new("x/./y")), normalize(Path::new("x/y")));
        // 越过根时钳制在根，后续由项目内前缀检查拒绝。
        assert_eq!(
            normalize(Path::new("/p/../../b")),
            normalize(Path::new("/b"))
        );
    }
}
