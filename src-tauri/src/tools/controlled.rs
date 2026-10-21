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
const MAX_OUTPUT_CHARS: usize = 24_000;
const MAX_WRITE_CHARS: usize = 16_000;

pub fn definitions() -> Vec<Value> {
    let schema = |name: &str, description: &str, properties: Value, required: Vec<&str>| {
        json!({"type":"function","function":{"name":name,"description":description,
            "parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}})
    };
    vec![
        schema(
            "update_task_state",
            "简短记录已完成事项、待办及摘要。全部用户要求完成时，将 pending 设为空、ready_for_review 设为 true，程序直接验证并汇总，无需额外一轮正文。无法验证不等于代码错误。",
            json!({"completed":{"type":"array","items":{"type":"string"},"maxItems":16},
                "pending":{"type":"array","items":{"type":"string"},"maxItems":16},
                "ready_for_review":{"type":"boolean"},"summary":{"type":"string","maxLength":32000},
                "task_kind":{"type":"string","enum":["analysis","coding","operation"]}}),
            vec!["completed", "pending", "ready_for_review", "summary"],
        ),
        schema(
            "run_verify",
            "运行本项目已检测到并授权的检查；不接受自定义命令。status=failed 才是代码检查失败；unavailable 表示无法检查，无需为此修改代码。",
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
            "read_files",
            "一次读取 1 至 8 个已知路径的 UTF-8 文件开头，返回各文件 hash 和正文。总输出有上限；用各项 next_offset 继续 read_file。单项失败不阻止其他文件读取。",
            json!({"paths":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":8}}),
            vec!["paths"],
        ),
        schema(
            "search",
            "按字面量搜索项目源码。",
            json!({"query":{"type":"string"}}),
            vec!["query"],
        ),
        schema(
            "search_symbols",
            "按符号名检索任务副本的当前源码（TS/JS/Python/Rust/Go/Java），返回声明位置及文件 hash。path 可为文件、目录或 .；大项目请缩小目录。",
            json!({"query":{"type":"string"},"path":{"type":"string"}}),
            vec!["query", "path"],
        ),
        schema(
            "read_symbol",
            "读取当前符号声明的正文及完整文件 hash，可用于 edit_file。start_line 使用搜索结果；唯一同名声明可填 0。offset 是符号内字符偏移，从 0 开始，按 next_offset 分块读取。",
            json!({"path":{"type":"string"},"name":{"type":"string"},"start_line":{"type":"integer","minimum":0},"offset":{"type":"integer","minimum":0}}),
            vec!["path", "name", "start_line", "offset"],
        ),
        schema(
            "create_file",
            "仅创建不存在的文件。长文件先创建小骨架，再用 append_file 分块追加；单次 content 最多 16000 字符。",
            json!({"path":{"type":"string"},"content":{"type":"string"}}),
            vec!["path", "content"],
        ),
        schema(
            "append_file",
            "用最新 hash 校验版本，在文件末尾追加 content，单次最多 16000 字符。返回新 hash。",
            json!({"path":{"type":"string"},"expected_hash":{"type":"string"},"content":{"type":"string"}}),
            vec!["path", "expected_hash", "content"],
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
struct ReadFilesArgs {
    paths: Vec<String>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppendArgs {
    path: String,
    expected_hash: String,
    content: String,
}

fn check_write_chunk(content: &str) -> Result<()> {
    if content.chars().count() > MAX_WRITE_CHARS {
        return Err(XaimError::Config(
            "单次写入超过 16000 字符，请分块创建、追加或编辑".into(),
        ));
    }
    Ok(())
}

fn read(root: &std::path::Path, path: &str) -> Result<String> {
    let target = safe_path(root, path)?;
    if fs::metadata(&target)?.len() > MAX_FILE_SIZE {
        return Err(XaimError::Security("文件超过 1 MB 工具上限".into()));
    }
    Ok(fs::read_to_string(target)?)
}

fn read_excerpt(path: &str, content: &str, offset: usize, budget: usize) -> Result<Value> {
    let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
    let length = content.chars().count();
    if offset > length {
        return Err(XaimError::Config("读取 offset 超出文件".into()));
    }
    let excerpt: Vec<char> = content.chars().skip(offset).take(MAX_READ_CHARS).collect();
    let value = |count: usize| {
        let next = offset + count;
        json!({"path":path,"hash":hash,"content":excerpt[..count].iter().collect::<String>(),
            "next_offset":next,"complete":next == length})
    };
    let mut low = 0;
    let mut high = excerpt.len();
    if value(0).to_string().chars().count() > budget {
        return Err(XaimError::Config("读取结果元数据超过输出上限".into()));
    }
    while low < high {
        let middle = (low + high).div_ceil(2);
        if value(middle).to_string().chars().count() <= budget {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    if low == 0 && !excerpt.is_empty() {
        return Err(XaimError::Config("读取结果无法容纳正文".into()));
    }
    Ok(value(low))
}

pub async fn call(name: &str, args: Value, ctx: &ToolContext) -> crate::types::ToolResult {
    if matches!(name, "search_symbols" | "read_symbol") {
        return super::symbols::SessionSymbols::default()
            .call(name, args, &ctx.project_root)
            .await;
    }
    let start = Instant::now();
    let outcome = execute(name, args, ctx).await;
    let result = outcome.unwrap_or_else(|error| ToolResult::fail(error.to_string()));
    crate::types::ToolResult {
        name: name.into(),
        success: result.success,
        output: if matches!(
            name,
            "read_file" | "read_files" | "create_file" | "edit_file" | "append_file"
        ) {
            result.output
        } else {
            super::exec::truncate_middle(&result.output, MAX_OUTPUT_CHARS)
        },
        error: result.error,
        duration_ms: Some(start.elapsed().as_millis() as i64),
    }
}

async fn execute(name: &str, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
    let root = &ctx.project_root;
    match name {
        "read_files" => {
            let args: ReadFilesArgs = serde_json::from_value(args)?;
            if args.paths.is_empty() || args.paths.len() > 8 {
                return Err(XaimError::Config("批量读取需要 1 至 8 个文件路径".into()));
            }
            // Bound the serialized envelope too, including escaping and long paths.
            let budget = (MAX_OUTPUT_CHARS - 512) / args.paths.len();
            let mut files = Vec::new();
            for (index, path) in args.paths.into_iter().enumerate() {
                if path.chars().count() > 1024 {
                    return Err(XaimError::Config("批量读取路径超过 1024 字符".into()));
                }
                let mut value = match read(root, &path)
                    .and_then(|content| read_excerpt(&path, &content, 0, budget))
                {
                    Ok(value) => value,
                    Err(error) => {
                        json!({"path":path,"error":error.to_string().chars().take(128).collect::<String>()})
                    }
                };
                if value.to_string().chars().count() > budget {
                    value = json!({"index":index,"error":"路径元数据超过批量输出上限，请单独 read_file"});
                }
                files.push(value);
            }
            Ok(ToolResult::ok(json!({"files":files}).to_string()))
        }
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
                // safe_path canonicalizes existing directories. On Windows its
                // extended path prefix can differ from the caller's root.
                let relative = if args.path == "." {
                    name.clone()
                } else {
                    format!("{}/{}", args.path.replace('\\', "/"), name)
                };
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
            Ok(ToolResult::ok(
                read_excerpt(&args.path, &content, args.offset, MAX_OUTPUT_CHARS)?.to_string(),
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
            check_write_chunk(&args.content)?;
            if args.content.trim().is_empty() || args.content.len() as u64 > MAX_FILE_SIZE {
                return Err(XaimError::Config("新文件内容为空或超过 1 MB".into()));
            }
            let target = safe_path(root, &args.path)?;
            fs::create_dir_all(target.parent().unwrap())?;
            // create_new is atomic and cannot overwrite an existing file; a
            // conflict is reported with the next step instead of a raw OS error.
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        XaimError::PatchConflict(format!(
                            "{}: 文件已存在，拒绝覆盖；请先 read_file 读取版本，再用 edit_file 修改，或改用 append_file",
                            args.path
                        ))
                    } else {
                        XaimError::Io(error)
                    }
                })?;
            file.write_all(args.content.as_bytes())?;
            Ok(ToolResult::ok(json!({"path":args.path,"hash":blake3::hash(args.content.as_bytes()).to_hex().to_string()}).to_string()))
        }
        "edit_file" => {
            let args: EditArgs = serde_json::from_value(args)?;
            check_write_chunk(&args.new_text)?;
            let content = read(root, &args.path)?;
            if blake3::hash(content.as_bytes()).to_hex().as_str() != args.expected_hash {
                return Err(XaimError::PatchConflict(
                    "文件版本已变化，请重新读取后编辑".into(),
                ));
            }
            if args.old_text.is_empty() {
                return Err(XaimError::PatchConflict(
                    "old_text 不能为空；请提供实际读取的待替换文本".into(),
                ));
            }
            match content.match_indices(&args.old_text).count() {
                0 => {
                    return Err(XaimError::PatchConflict(
                        "old_text 未匹配；请使用读取结果中的原文，保留空格与换行".into(),
                    ))
                }
                1 => {}
                count => {
                    return Err(XaimError::PatchConflict(format!(
                        "old_text 匹配 {count} 处；请增加前后文以唯一定位"
                    )))
                }
            }
            let updated = content.replacen(&args.old_text, &args.new_text, 1);
            if updated == content {
                return Ok(ToolResult::ok(
                    json!({"path":args.path,"hash":args.expected_hash,"changed":false}).to_string(),
                ));
            }
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
        "append_file" => {
            let args: AppendArgs = serde_json::from_value(args)?;
            check_write_chunk(&args.content)?;
            if args.content.is_empty() {
                return Err(XaimError::Config("追加内容不能为空".into()));
            }
            let content = read(root, &args.path)?;
            if blake3::hash(content.as_bytes()).to_hex().as_str() != args.expected_hash {
                return Err(XaimError::PatchConflict(
                    "文件版本已变化，请重新读取后追加".into(),
                ));
            }
            let updated = format!("{content}{}", args.content);
            if updated.len() as u64 > MAX_FILE_SIZE {
                return Err(XaimError::Security("追加结果超过 1 MB".into()));
            }
            crate::agent::workspace::ChangeSet {
                files: vec![crate::agent::workspace::FileChange {
                    path: args.path.clone(),
                    before: Some(content),
                    after: Some(updated.clone()),
                }],
            }
            .apply(root, false)?;
            Ok(ToolResult::ok(json!({"path":args.path,"hash":blake3::hash(updated.as_bytes()).to_hex().to_string()}).to_string()))
        }
        _ => Err(XaimError::Security(format!("未授权工具: {name}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_file_reports_an_actionable_message_when_the_file_exists() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("page.html"), "<!DOCTYPE html>").unwrap();
        let ctx = ToolContext {
            project_root: root.path().into(),
            store: std::sync::Arc::new(crate::store::Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        };
        let result = call(
            "create_file",
            json!({"path":"page.html","content":"new"}),
            &ctx,
        )
        .await;
        assert!(!result.success);
        let message = result.error.unwrap_or_default();
        assert!(message.contains("文件已存在"), "{message}");
        assert!(message.contains("append_file"), "{message}");
        assert_eq!(
            fs::read_to_string(root.path().join("page.html")).unwrap(),
            "<!DOCTYPE html>"
        );
    }

    #[tokio::test]
    async fn batch_reads_bound_escaped_output_keep_hashes_and_isolate_errors() {
        let root = tempfile::tempdir().unwrap();
        let content = format!("{}尾部", "\"\\\n中文".repeat(4000));
        fs::write(root.path().join("large.txt"), &content).unwrap();
        fs::write(root.path().join("small.txt"), "ready").unwrap();
        fs::write(root.path().join(".env"), "PRIVATE_SECRET").unwrap();
        let ctx = ToolContext {
            project_root: root.path().into(),
            store: std::sync::Arc::new(crate::store::Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        };
        let result = call("read_files", json!({"paths":["large.txt","small.txt","missing.txt",".env","../outside","large.txt","small.txt","large.txt"]}), &ctx).await;
        assert!(result.success, "{result:?}");
        assert!(result.output.chars().count() <= MAX_OUTPUT_CHARS);
        assert!(!result.output.contains("PRIVATE_SECRET"));
        let batch: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(batch["files"].as_array().unwrap().len(), 8);
        assert_eq!(batch["files"][1]["content"], "ready");
        for index in [2, 3, 4] {
            assert!(batch["files"][index]["error"].is_string());
            assert!(batch["files"][index]["content"].is_null());
        }
        let first = &batch["files"][0];
        assert_eq!(
            first["hash"],
            blake3::hash(content.as_bytes()).to_hex().to_string()
        );
        assert_eq!(first["complete"], false);
        let mut recovered = first["content"].as_str().unwrap().to_string();
        let mut offset = first["next_offset"].as_u64().unwrap();
        loop {
            let response = call(
                "read_file",
                json!({"path":"large.txt","offset":offset}),
                &ctx,
            )
            .await;
            assert!(response.success);
            let chunk: Value = serde_json::from_str(&response.output).unwrap();
            recovered.push_str(chunk["content"].as_str().unwrap());
            if chunk["complete"] == true {
                break;
            }
            offset = chunk["next_offset"].as_u64().unwrap();
        }
        assert_eq!(recovered, content);
        for paths in [Vec::<String>::new(), vec!["small.txt".into(); 9]] {
            assert!(
                !call("read_files", json!({"paths":paths}), &ctx)
                    .await
                    .success
            );
        }
        let edited = call("edit_file", json!({"path":"small.txt","expected_hash":batch["files"][1]["hash"],"old_text":"ready","new_text":"done"}), &ctx).await;
        assert!(edited.success);
        assert!(!call("edit_file", json!({"path":"small.txt","expected_hash":batch["files"][1]["hash"],"old_text":"done","new_text":"done"}), &ctx).await.success);
    }

    #[tokio::test]
    async fn list_nested_directory_with_noncanonical_root() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/lib.rs"), "pub fn login() {}\n").unwrap();
        let ctx = ToolContext {
            project_root: root.path().to_path_buf(),
            store: std::sync::Arc::new(crate::store::Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        };
        let result = call("list_dir", json!({"path":"src"}), &ctx).await;
        assert!(result.success, "{result:?}");
        assert_eq!(result.output, "lib.rs");
    }

    #[tokio::test]
    async fn escaped_reads_remain_valid_and_chunks_reconstruct_exactly() {
        let root = tempfile::tempdir().unwrap();
        let original = format!("{}尾部", "\"\\\n".repeat(8000));
        fs::write(root.path().join("a"), &original).unwrap();
        let ctx = ToolContext {
            project_root: root.path().into(),
            store: std::sync::Arc::new(crate::store::Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        };
        let mut offset = 0;
        let mut recovered = String::new();
        loop {
            let response = call("read_file", json!({"path":"a","offset":offset}), &ctx).await;
            assert!(response.success);
            assert!(response.output.chars().count() <= MAX_OUTPUT_CHARS);
            let result: Value = serde_json::from_str(&response.output).unwrap();
            let chunk = result["content"].as_str().unwrap();
            let next = result["next_offset"].as_u64().unwrap() as usize;
            assert_eq!(next, offset + chunk.chars().count());
            recovered.push_str(chunk);
            if result["complete"] == true {
                break;
            }
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(recovered, original);
        let created = call("create_file", json!({"path":"b","content":"start"}), &ctx).await;
        let result: Value = serde_json::from_str(&created.output).unwrap();
        assert!(
            !call(
                "append_file",
                json!({"path":"b","expected_hash":"stale","content":"end"}),
                &ctx
            )
            .await
            .success
        );
        let appended = call(
            "append_file",
            json!({"path":"b","expected_hash":result["hash"],"content":"end"}),
            &ctx,
        )
        .await;
        assert!(appended.success);
        assert_eq!(
            fs::read_to_string(root.path().join("b")).unwrap(),
            "startend"
        );
        let result: Value = serde_json::from_str(&appended.output).unwrap();
        assert!(!call("append_file", json!({"path":"b","expected_hash":result["hash"],"content":"x".repeat(MAX_WRITE_CHARS + 1)}), &ctx).await.success);
        assert_eq!(
            fs::read_to_string(root.path().join("b")).unwrap(),
            "startend"
        );
    }

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
