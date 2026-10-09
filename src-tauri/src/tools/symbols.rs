//! Task-local Tree-sitter cache. Hashes are checked against the working copy on
//! every request, so edits, additions and deletions cannot leave stale symbols.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::workspace::{excluded, safe_path};
use crate::errors::{Result, XaimError};
use crate::index::{
    lang::detect_language,
    symbol::{extract_symbols, Symbol},
};
use crate::types::ToolResult;

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_SCAN_BYTES: usize = 32 * 1024 * 1024;
const MAX_FILES: usize = 50_000;
const MAX_OUTPUT_BYTES: usize = 24_000;

#[derive(Default, Clone)]
pub struct SessionSymbols(Arc<Mutex<BTreeMap<String, Cached>>>);
struct Cached {
    hash: String,
    symbols: Vec<Symbol>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    query: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    path: String,
    name: String,
    start_line: u32,
    offset: usize,
}

impl SessionSymbols {
    pub async fn call(&self, name: &str, args: Value, root: &Path) -> ToolResult {
        let started = Instant::now();
        let cache = self.clone();
        let root = root.to_path_buf();
        let tool = name.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            let root = root.canonicalize()?;
            cache.execute(&tool, args, &root)
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => Err(XaimError::Unknown(format!("符号检索线程失败: {error}"))),
        };
        let (output, error) = match result {
            Ok(output) => (output, None),
            Err(error) => (String::new(), Some(error.to_string())),
        };
        ToolResult {
            name: name.into(),
            success: error.is_none(),
            output,
            error,
            duration_ms: Some(started.elapsed().as_millis() as i64),
        }
    }

    fn parse(&self, root: &Path, path: &str) -> Result<(String, String, Vec<Symbol>)> {
        let target = safe_path(root, path)?;
        if fs::metadata(&target)?.len() > MAX_FILE_BYTES {
            return Err(XaimError::Security("文件超过 1 MB 符号工具上限".into()));
        }
        let content = fs::read_to_string(target)?;
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();
        let mut cache = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if !cache.get(path).is_some_and(|entry| entry.hash == hash) {
            let symbols =
                extract_symbols(Path::new(path), &detect_language(Path::new(path)), &content)?;
            cache.insert(
                path.into(),
                Cached {
                    hash: hash.clone(),
                    symbols,
                },
            );
        }
        Ok((content, hash, cache[path].symbols.clone()))
    }

    fn execute(&self, name: &str, args: Value, root: &Path) -> Result<String> {
        match name {
            "search_symbols" => {
                let args: Search = serde_json::from_value(args)?;
                if args.query.trim().is_empty() || args.query.chars().count() > 256 {
                    return Err(XaimError::Config(
                        "query 必须为非空字符串且不超过 256 字符".into(),
                    ));
                }
                let target = if args.path == "." {
                    root.to_path_buf()
                } else {
                    safe_path(root, &args.path)?
                };
                if !target.exists() {
                    return Err(XaimError::Config("符号搜索路径不存在".into()));
                }
                let mut paths: Vec<PathBuf> = Vec::new();
                let mut incomplete = false;
                for entry in ignore::WalkBuilder::new(target)
                    .hidden(false)
                    .parents(false)
                    .require_git(false)
                    .git_global(false)
                    .follow_links(false)
                    .filter_entry(|entry| !excluded(&entry.file_name().to_string_lossy()))
                    .sort_by_file_path(|a, b| a.cmp(b))
                    .build()
                {
                    let Ok(entry) = entry else {
                        incomplete = true;
                        continue;
                    };
                    if entry.file_type().is_some_and(|kind| kind.is_file()) {
                        if paths.len() >= MAX_FILES {
                            incomplete = true;
                            break;
                        }
                        paths.push(entry.path().into());
                    }
                }
                let mut found = Vec::new();
                let mut total = 0;
                let mut scanned_bytes = 0;
                let query = args.query.trim().to_lowercase();
                for target in paths {
                    if !matches!(
                        detect_language(&target).as_str(),
                        "typescript" | "javascript" | "python" | "rust" | "go" | "java"
                    ) {
                        continue;
                    }
                    let Ok(relative) = target.strip_prefix(root) else {
                        incomplete = true;
                        continue;
                    };
                    let path = relative.to_string_lossy().replace('\\', "/");
                    if safe_path(root, &path).is_err() {
                        continue;
                    }
                    let Ok(metadata) = fs::metadata(&target) else {
                        incomplete = true;
                        continue;
                    };
                    if metadata.len() > MAX_FILE_BYTES {
                        incomplete = true;
                        continue;
                    }
                    if scanned_bytes + metadata.len() as usize > MAX_SCAN_BYTES {
                        incomplete = true;
                        break;
                    }
                    scanned_bytes += metadata.len() as usize;
                    let Ok((_, hash, symbols)) = self.parse(root, &path) else {
                        incomplete = true;
                        continue;
                    };
                    for symbol in symbols {
                        let symbol_name = symbol.name.to_lowercase();
                        if !symbol_name.contains(&query) {
                            continue;
                        }
                        let rank = if symbol_name == query {
                            0
                        } else if symbol_name.starts_with(&query) {
                            1
                        } else {
                            2
                        };
                        total += 1;
                        found.push((rank, symbol.name.len(), symbol.file.clone(), symbol.start_line,
                            json!({"name":symbol.name,"kind":symbol.kind,"path":symbol.file,
                                "start_line":symbol.start_line,"end_line":symbol.end_line,"hash":hash})));
                        found.sort_by(|a, b| {
                            (&a.0, &a.1, &a.2, &a.3).cmp(&(&b.0, &b.1, &b.2, &b.3))
                        });
                        found.truncate(50);
                    }
                }
                found.sort_by(|a, b| (&a.0, &a.1, &a.2, &a.3).cmp(&(&b.0, &b.1, &b.2, &b.3)));
                let mut results: Vec<Value> =
                    found.into_iter().take(50).map(|entry| entry.4).collect();
                loop {
                    let output =
                        json!({"results":results,"matches":total,"truncated":results.len()<total,
                        "incomplete":incomplete,"scanned_bytes":scanned_bytes,
                        "note":"结果只覆盖支持的语言；incomplete=true 时请缩小 path 搜索范围"})
                        .to_string();
                    if output.len() <= MAX_OUTPUT_BYTES {
                        return Ok(output);
                    }
                    results.pop();
                }
            }
            "read_symbol" => {
                let args: Read = serde_json::from_value(args)?;
                let (content, hash, symbols) = self.parse(root, &args.path)?;
                let matches: Vec<_> = symbols
                    .iter()
                    .filter(|s| {
                        s.name == args.name
                            && (args.start_line == 0 || s.start_line == args.start_line)
                    })
                    .collect();
                if matches.len() != 1 {
                    return Err(XaimError::Config("符号不存在或有多个同名声明，请先 search_symbols，使用最新 start_line；唯一声明可填 0".into()));
                }
                let symbol = matches[0];
                let start = content
                    .split_inclusive('\n')
                    .take(symbol.start_line.saturating_sub(1) as usize)
                    .map(str::len)
                    .sum::<usize>();
                let end = content
                    .split_inclusive('\n')
                    .take(symbol.end_line as usize)
                    .map(str::len)
                    .sum::<usize>();
                let chars: Vec<char> = content[start..end].chars().collect();
                if args.offset > chars.len() {
                    return Err(XaimError::Config("offset 超过符号正文长度".into()));
                }
                let mut count = (chars.len() - args.offset).min(8000);
                loop {
                    let output = json!({"path":args.path,"name":args.name,"hash":hash,
                        "start_line":symbol.start_line,"end_line":symbol.end_line,
                        "content":chars[args.offset..args.offset+count].iter().collect::<String>(),
                        "next_offset":args.offset+count,"complete":args.offset+count==chars.len()})
                    .to_string();
                    if output.len() <= MAX_OUTPUT_BYTES {
                        return Ok(output);
                    }
                    if count == 0 {
                        return Err(XaimError::Config("符号元数据过长".into()));
                    }
                    count /= 2;
                }
            }
            _ => Err(XaimError::Security("未知符号工具".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scoped_search_with_noncanonical_root() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/lib.rs"), "pub fn login() {}\n").unwrap();
        let tools = SessionSymbols::default();
        for path in ["src", "src/lib.rs"] {
            let result = tools
                .call(
                    "search_symbols",
                    json!({"query":"login","path":path}),
                    root.path(),
                )
                .await;
            assert!(result.success, "{result:?}");
            let found: Value = serde_json::from_str(&result.output).unwrap();
            assert_eq!(found["matches"], 1);
            assert_eq!(found["results"][0]["path"], "src/lib.rs");
        }
    }

    #[tokio::test]
    async fn search_and_read_follow_edits_additions_deletions_and_ambiguity() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("lib.rs"), "pub fn login() {}\n").unwrap();
        let tools = SessionSymbols::default();
        let result = tools
            .call(
                "search_symbols",
                json!({"query":"login","path":"."}),
                root.path(),
            )
            .await;
        assert!(result.success);
        let found: Value = serde_json::from_str(&result.output).unwrap();
        let result = tools
            .call(
                "read_symbol",
                json!({"path":"lib.rs","name":"login","start_line":1,"offset":0}),
                root.path(),
            )
            .await;
        let read: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(found["results"][0]["hash"], read["hash"]);
        assert_eq!(read["content"], "pub fn login() {}\n");
        fs::write(root.path().join("lib.rs"), "pub fn logout() {}\n").unwrap();
        let result = tools
            .call(
                "search_symbols",
                json!({"query":"login","path":"."}),
                root.path(),
            )
            .await;
        assert_eq!(
            serde_json::from_str::<Value>(&result.output).unwrap()["matches"],
            0
        );
        fs::write(root.path().join("new.py"), "def login():\n    return 1\n\nclass Service:\n    def login(self):\n        return 2\n").unwrap();
        let ambiguous = tools
            .call(
                "read_symbol",
                json!({"path":"new.py","name":"login","start_line":0,"offset":0}),
                root.path(),
            )
            .await;
        assert!(!ambiguous.success);
        let result = tools
            .call(
                "read_symbol",
                json!({"path":"new.py","name":"login","start_line":5,"offset":0}),
                root.path(),
            )
            .await;
        assert!(result.success, "{result:?}");
        assert!(result.output.contains("return 2"));
        fs::remove_file(root.path().join("new.py")).unwrap();
        let result = tools
            .call(
                "search_symbols",
                json!({"query":"login","path":"."}),
                root.path(),
            )
            .await;
        assert_eq!(
            serde_json::from_str::<Value>(&result.output).unwrap()["matches"],
            0
        );
        assert!(
            !tools
                .call(
                    "read_symbol",
                    json!({"path":"../lib.rs","name":"login","start_line":0,"offset":0}),
                    root.path()
                )
                .await
                .success
        );
    }

    #[tokio::test]
    async fn chunked_symbol_reads_preserve_unicode_and_sensitive_paths_are_excluded() {
        let root = tempfile::tempdir().unwrap();
        let body = format!("pub fn login() {{\n{}\n}}\n", "// 中文\\\"\n".repeat(4000));
        fs::write(root.path().join("lib.rs"), &body).unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        fs::write(
            root.path().join(".git/secret.rs"),
            "pub fn login_secret() {}\n",
        )
        .unwrap();
        let tools = SessionSymbols::default();
        let search = tools
            .call(
                "search_symbols",
                json!({"query":"login","path":"."}),
                root.path(),
            )
            .await;
        assert!(!search.output.contains("login_secret"));
        let mut content = String::new();
        let mut offset = 0;
        loop {
            let result = tools
                .call(
                    "read_symbol",
                    json!({"path":"lib.rs","name":"login","start_line":0,"offset":offset}),
                    root.path(),
                )
                .await;
            assert!(result.success);
            assert!(result.output.len() <= MAX_OUTPUT_BYTES);
            let chunk: Value = serde_json::from_str(&result.output).unwrap();
            content.push_str(chunk["content"].as_str().unwrap());
            if chunk["complete"] == true {
                break;
            }
            let next = chunk["next_offset"].as_u64().unwrap() as usize;
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(content, body);
    }
}
