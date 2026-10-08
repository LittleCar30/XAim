//! Only these file tools are exposed to the task executor; no arbitrary commands.

use std::fs;
use std::io::Write;
use std::time::Instant;

use serde::Deserialize;
use serde_json::{json, Value};

use super::r#trait::{Tool, ToolContext, ToolResult};
use crate::agent::workspace::{excluded, safe_path};
use crate::errors::{Result, XaimError};

const MAX_FILE_SIZE: u64 = 1024 * 1024;
const MAX_READ_CHARS: usize = 16_000;

pub fn definitions() -> Vec<Value> {
    let schema = |name: &str, description: &str, properties: Value, required: Vec<&str>| {
        json!({"type":"function","function":{"name":name,"description":description,
            "parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}})
    };
    vec![
        schema(
            "run_verify",
            "运行本项目已检测到并授权的检查；不接受自定义命令。",
            json!({}),
            vec![],
        ),
        schema(
            "list_dir",
            "列出隔离任务目录中的文件和目录。",
            json!({"path":{"type":"string"}}),
            vec!["path"],
        ),
        schema(
            "read_file",
            "读取 UTF-8 文件，返回版本 hash。长文件用 offset 分段读取。",
            json!({"path":{"type":"string"},"offset":{"type":"integer","minimum":0}}),
            vec!["path", "offset"],
        ),
        schema(
            "search",
            "按字面量搜索项目源码。",
            json!({"query":{"type":"string"}}),
            vec!["query"],
        ),
        schema(
            "create_file",
            "仅创建不存在的文件，content 是完整源码。",
            json!({"path":{"type":"string"},"content":{"type":"string"}}),
            vec!["path", "content"],
        ),
        schema(
            "edit_file",
            "以 read_file 返回的 hash 校验版本，将唯一匹配的 old_text 替换为 new_text。",
            json!({"path":{"type":"string"},"expected_hash":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}}),
            vec!["path", "expected_hash", "old_text", "new_text"],
        ),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    path: String,
    offset: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    path: String,
    content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    path: String,
    expected_hash: String,
    old_text: String,
    new_text: String,
}

fn read(root: &std::path::Path, path: &str) -> Result<String> {
    let target = safe_path(root, path)?;
    if fs::metadata(&target)?.len() > MAX_FILE_SIZE {
        return Err(XaimError::Security("文件超过 1 MB 工具上限".into()));
    }
    Ok(fs::read_to_string(target)?)
}

pub async fn call(name: &str, args: Value, ctx: &ToolContext) -> crate::types::ToolResult {
    let start = Instant::now();
    let outcome = execute(name, args, ctx).await;
    let result = outcome.unwrap_or_else(|error| ToolResult::fail(error.to_string()));
    crate::types::ToolResult {
        name: name.into(),
        success: result.success,
        output: super::exec::truncate_middle(&result.output, 24_000),
        error: result.error,
        duration_ms: Some(start.elapsed().as_millis() as i64),
    }
}

async fn execute(name: &str, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
    let root = &ctx.project_root;
    match name {
        "list_dir" => {
            let args: PathArgs = serde_json::from_value(args)?;
            let dir = if args.path == "." {
                root.clone()
            } else {
                safe_path(root, &args.path)?
            };
            let mut entries = vec![];
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if excluded(&name) {
                    continue;
                }
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if safe_path(root, &relative).is_err() {
                    continue;
                }
                entries.push(format!(
                    "{name}{}",
                    if entry.file_type()?.is_dir() { "/" } else { "" }
                ));
                if entries.len() >= 500 {
                    break;
                }
            }
            entries.sort();
            Ok(ToolResult::ok(entries.join("\n")))
        }
        "read_file" => {
            let args: ReadArgs = serde_json::from_value(args)?;
            let content = read(root, &args.path)?;
            let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
            let length = content.chars().count();
            if args.offset > length {
                return Err(XaimError::Config("读取 offset 超出文件".into()));
            }
            let excerpt: String = content
                .chars()
                .skip(args.offset)
                .take(MAX_READ_CHARS)
                .collect();
            let next = args.offset + excerpt.chars().count();
            Ok(ToolResult::ok(
                json!({"path":args.path,"hash":hash,"content":excerpt,
                "next_offset":next,"complete": next == length})
                .to_string(),
            ))
        }
        "search" => {
            let args: SearchArgs = serde_json::from_value(args)?;
            super::search::SearchTool
                .call(json!({"query":args.query,"max_results":50}), ctx)
                .await
        }
        "create_file" => {
            let args: CreateArgs = serde_json::from_value(args)?;
            if args.content.trim().is_empty() || args.content.len() as u64 > MAX_FILE_SIZE {
                return Err(XaimError::Config("新文件内容为空或超过 1 MB".into()));
            }
            let target = safe_path(root, &args.path)?;
            fs::create_dir_all(target.parent().unwrap())?;
            // create_new is atomic and cannot overwrite an existing file.
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)?;
            file.write_all(args.content.as_bytes())?;
            Ok(ToolResult::ok(json!({"path":args.path,"hash":blake3::hash(args.content.as_bytes()).to_hex().to_string()}).to_string()))
        }
        "edit_file" => {
            let args: EditArgs = serde_json::from_value(args)?;
            let content = read(root, &args.path)?;
            if blake3::hash(content.as_bytes()).to_hex().as_str() != args.expected_hash {
                return Err(XaimError::PatchConflict(
                    "文件版本已变化，请重新读取后编辑".into(),
                ));
            }
            if args.old_text.is_empty() || content.match_indices(&args.old_text).count() != 1 {
                return Err(XaimError::PatchConflict(
                    "old_text 必须非空且唯一匹配；请重新读取并提供更多上下文".into(),
                ));
            }
            let updated = content.replacen(&args.old_text, &args.new_text, 1);
            if updated.len() as u64 > MAX_FILE_SIZE {
                return Err(XaimError::Security("编辑结果超过 1 MB".into()));
            }
            let changes = crate::agent::workspace::ChangeSet {
                files: vec![crate::agent::workspace::FileChange {
                    path: args.path.clone(),
                    before: Some(content),
                    after: Some(updated.clone()),
                }],
            };
            changes.apply(root, false)?;
            Ok(ToolResult::ok(json!({"path":args.path,"hash":blake3::hash(updated.as_bytes()).to_hex().to_string()}).to_string()))
        }
        _ => Err(XaimError::Security(format!("未授权工具: {name}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn edits_require_unique_nonempty_match_and_current_version() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a"), "one one two").unwrap();
        let ctx = ToolContext {
            project_root: root.path().into(),
            store: std::sync::Arc::new(crate::store::Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        };
        let response = call("read_file", json!({"path":"a","offset":0}), &ctx).await;
        let read: Value = serde_json::from_str(&response.output).unwrap();
        for (old, hash) in [
            ("", read["hash"].as_str().unwrap()),
            ("one", read["hash"].as_str().unwrap()),
            ("two", "stale"),
        ] {
            assert!(
                !call(
                    "edit_file",
                    json!({"path":"a","expected_hash":hash,"old_text":old,"new_text":"new"}),
                    &ctx
                )
                .await
                .success
            );
        }
        assert!(
            call(
                "edit_file",
                json!({"path":"a","expected_hash":read["hash"],"old_text":"two","new_text":"new"}),
                &ctx
            )
            .await
            .success
        );
        assert!(
            !call(
                "create_file",
                json!({"path":"a","content":"overwrite"}),
                &ctx
            )
            .await
            .success
        );
        assert_eq!(
            fs::read_to_string(root.path().join("a")).unwrap(),
            "one one new"
        );
        assert!(
            !call("run_command", json!({"command":"git reset --hard"}), &ctx)
                .await
                .success
        );
    }
}
