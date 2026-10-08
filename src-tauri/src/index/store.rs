//! 索引管线：扫描 → 哈希/语言/符号 → 写入 `files` / `symbols` 表。
//!
//! 把 [`scanner`] / [`hash`] / [`lang`] / [`symbol`] 串成增量索引的存储层：
//! - 重活（扫描、读文件、解析）在**不持有 DB 锁**的情况下完成，最后只在
//!   单个事务内批量写入并清理失效记录，避免长时间独占 [`Store`] 连接；
//! - 哈希未变化的文件跳过写入（重复全量索引近似增量）；
//! - 大项目用 `index_project_async*` 放到阻塞线程池，不阻塞 UI/主线程；
//!   进度经 [`IndexProgress`] 回调上报（命令层负责转成前端事件）。
//!
//! 路径约定：`files.path` 存**相对项目根、`/` 分隔**的路径；`update_file_index` /
//  `remove_file_index` 的 `path` 参数既可传相对路径（来自文件树）也可传绝对路径
//! （来自 watcher 事件），内部统一归一化。`get_stale_files` 返回相对路径。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{fs, time::UNIX_EPOCH};

use rusqlite::{params, Connection};

use crate::errors::{Result, XaimError};
use crate::index::symbol::Symbol;
use crate::index::{hash, lang, scanner, symbol};
use crate::store::{file, project, Store};
use crate::types::FileNode;

/// 索引进度：已处理 `indexed` / 共 `total` 个文件，当前为 `path`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexProgress {
    pub indexed: usize,
    pub total: usize,
    pub path: String,
}

/// 全量索引项目（无进度上报）。见 [`index_project_with_progress`]。
pub fn index_project(store: &Store, project_id: i64, root: &Path) -> Result<()> {
    index_project_with_progress(store, project_id, root, |_| {})
}

/// 全量索引项目，每处理完一个文件回调一次 [`IndexProgress`]。
///
/// 流程：扫描 → 逐文件计算哈希/语言/符号 → 单事务写入 `files` 与 `symbols`
/// → 删除磁盘上已不存在的记录。
pub fn index_project_with_progress(
    store: &Store,
    project_id: i64,
    root: &Path,
    progress: impl FnMut(IndexProgress),
) -> Result<()> {
    // 阶段 1：扫描（复用 scanner 的忽略规则，不持 DB 锁）。
    let tree = scanner::scan_project(root, &[])?;
    let mut entries: Vec<(String, u64)> = Vec::new();
    flatten(&tree, &mut entries);

    // 阶段 2：逐文件读内容/哈希/语言/符号（重活，不持 DB 锁）。
    let total = entries.len();
    let mut progress = progress;
    let mut indexed: Vec<FileEntry> = Vec::with_capacity(total);
    for (done, (path, _)) in entries.iter().enumerate() {
        if let Some(entry) = analyze(root, path) {
            indexed.push(entry);
        }
        progress(IndexProgress {
            indexed: done + 1,
            total,
            path: path.clone(),
        });
    }

    // 阶段 3+4：单次持锁，事务写入 + 清理失效记录。
    let conn = store.conn();
    let tx = conn.unchecked_transaction()?;

    let records = file::list_files(&tx, project_id)?;
    let existing: HashMap<&str, &str> = records
        .iter()
        .filter_map(|record| record.hash.as_deref().map(|h| (record.path.as_str(), h)))
        .collect();

    for entry in &indexed {
        if existing.get(entry.path.as_str()).copied() == Some(entry.hash.as_str()) {
            continue; // 内容未变：跳过写入，保留原记录与 symbols
        }
        let file_id = file::upsert_file(
            &tx,
            project_id,
            &entry.path,
            Some(&entry.language),
            Some(entry.size),
            Some(entry.mtime),
            Some(&entry.hash),
        )?;
        replace_symbols(&tx, file_id, &entry.symbols)?;
    }

    // 磁盘上已不存在（或不再可索引）的记录：删除，symbols 随外键级联清理。
    let scanned: std::collections::HashSet<&str> =
        entries.iter().map(|(path, _)| path.as_str()).collect();
    for record in &records {
        if !scanned.contains(record.path.as_str()) {
            file::delete_file(&tx, project_id, &record.path)?;
        }
    }

    tx.commit()?;
    Ok(())
}

/// 异步全量索引：放到 tokio 阻塞线程池，不阻塞 UI。见 [`index_project`]。
pub async fn index_project_async(store: Store, project_id: i64, root: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || index_project(&store, project_id, &root))
        .await
        .map_err(|err| XaimError::Unknown(format!("索引线程异常退出: {err}")))?
}

/// [`index_project_async`] 的带进度版本，见 [`index_project_with_progress`]。
pub async fn index_project_async_with_progress<F>(
    store: Store,
    project_id: i64,
    root: PathBuf,
    progress: F,
) -> Result<()>
where
    F: FnMut(IndexProgress) + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        index_project_with_progress(&store, project_id, &root, progress)
    })
    .await
    .map_err(|err| XaimError::Unknown(format!("索引线程异常退出: {err}")))?
}

/// 增量更新单个文件的索引（哈希、语言、符号）。
///
/// 供 watcher 的 Created/Modified 事件调用；内容哈希未变化时直接返回，
/// 不做重复解析。文件已不存在时返回 IO 错误（此时调用方应改用
/// [`remove_file_index`]）。
pub fn update_file_index(store: &Store, project_id: i64, path: &Path) -> Result<()> {
    let key = storage_key(store, project_id, path)?;
    let root = project_root(store, project_id)?;
    let abs = root.join(&key);

    let content = fs::read_to_string(&abs)?;
    let digest = hash::content_hash(&content);
    let language = lang::detect_language(&abs);

    // 内容未变：跳过（watcher 的元数据抖动不应引起重复索引）。
    {
        let conn = store.conn();
        if let Some(record) = file::get_file(&conn, project_id, &key)? {
            if record.hash.as_deref() == Some(digest.as_str()) {
                return Ok(());
            }
        }
    }

    let symbols = symbol::extract_symbols(&abs, &language, &content)?;
    let mtime = fs::metadata(&abs).ok().and_then(|meta| mtime_ms(&meta));

    let conn = store.conn();
    let tx = conn.unchecked_transaction()?;
    let file_id = file::upsert_file(
        &tx,
        project_id,
        &key,
        Some(&language),
        Some(content.len() as i64),
        mtime,
        Some(&digest),
    )?;
    replace_symbols(&tx, file_id, &symbols)?;
    tx.commit()?;
    Ok(())
}

/// 移除单个文件的索引记录；该文件的 symbols 随外键级联删除。
/// 目标不存在时不报错（影响 0 行）。
pub fn remove_file_index(store: &Store, project_id: i64, path: &Path) -> Result<()> {
    let key = storage_key(store, project_id, path)?;
    let conn = store.conn();
    file::delete_file(&conn, project_id, &key)
}

/// 找出索引与磁盘不再一致的已索引文件（相对路径）：
/// 磁盘上已删除的、或 mtime 与索引时不一致的。
///
/// 纯 mtime 启发式，不读内容，可低成本地做启动/定时对账；
/// 新建文件的发现由 [`index_project`] 与 watcher 负责，不在本函数范围。
/// 调用方对每个返回路径：磁盘存在 → [`update_file_index`]，否则 [`remove_file_index`]。
pub fn get_stale_files(store: &Store, project_id: i64) -> Result<Vec<PathBuf>> {
    let root = project_root(store, project_id)?;
    let records = {
        let conn = store.conn();
        file::list_files(&conn, project_id)?
    };

    let mut stale = Vec::new();
    for record in records {
        let abs = root.join(&record.path);
        let Ok(meta) = fs::metadata(&abs) else {
            stale.push(PathBuf::from(record.path)); // 磁盘已删除
            continue;
        };
        if mtime_ms(&meta) != record.mtime {
            stale.push(PathBuf::from(record.path));
        }
    }
    Ok(stale)
}

/// 分析单个文件：读内容、算哈希、识别语言、提取符号。
/// 不可读（非 UTF-8 / 被占用等）返回 `None`，不中断整体索引。
fn analyze(root: &Path, path: &str) -> Option<FileEntry> {
    let abs = root.join(path);
    let content = match fs::read_to_string(&abs) {
        Ok(content) => content,
        Err(err) => {
            tracing::debug!(path, %err, "索引跳过不可读文件");
            return None;
        }
    };
    let language = lang::detect_language(&abs);
    let hash = hash::content_hash(&content);
    let symbols = match symbol::extract_symbols(&abs, &language, &content) {
        Ok(symbols) => symbols,
        Err(err) => {
            tracing::warn!(path, %err, "符号提取失败，按无符号处理");
            Vec::new()
        }
    };
    let mtime = fs::metadata(&abs).ok().and_then(|meta| mtime_ms(&meta));
    Some(FileEntry {
        path: path.to_string(),
        hash,
        language,
        size: content.len() as i64,
        mtime: mtime.unwrap_or(0),
        symbols,
    })
}

/// 把语法树里的文件节点摊平成（相对路径, 大小）列表。
fn flatten(node: &FileNode, out: &mut Vec<(String, u64)>) {
    for child in &node.children {
        if child.is_dir {
            flatten(child, out);
        } else {
            out.push((child.path.clone(), child.size.unwrap_or(0)));
        }
    }
}

/// 用新符号整体替换某文件的旧符号（先删后插，调用方需在事务内）。
fn replace_symbols(conn: &Connection, file_id: i64, symbols: &[Symbol]) -> Result<()> {
    conn.execute("DELETE FROM symbols WHERE file_id = ?1", [file_id])?;
    let mut stmt = conn.prepare(
        "INSERT INTO symbols (file_id, name, kind, start_line, end_line)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for symbol in symbols {
        stmt.execute(params![
            file_id,
            symbol.name,
            symbol.kind,
            i64::from(symbol.start_line),
            i64::from(symbol.end_line),
        ])?;
    }
    Ok(())
}

/// 查项目根路径；项目不存在返回 [`XaimError::Config`]。
fn project_root(store: &Store, project_id: i64) -> Result<PathBuf> {
    let conn = store.conn();
    let Some(project) = project::get_project(&conn, project_id)? else {
        return Err(XaimError::Config(format!("项目 {project_id} 不存在")));
    };
    Ok(PathBuf::from(project.path))
}

/// 归一化 `path` 为存储键：绝对路径先剥掉项目根，统一转成相对路径、
/// `/` 分隔的字符串（与 [`scanner`] 产出、`files.path` 存储一致）。
fn storage_key(store: &Store, project_id: i64, path: &Path) -> Result<String> {
    let relative = if path.is_absolute() {
        let root = project_root(store, project_id)?;
        path.strip_prefix(&root).unwrap_or(path)
    } else {
        path
    };
    Ok(relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

/// 文件修改时间，毫秒级 `i64`；取不到（时钟早于纪元等）返回 `None`。
fn mtime_ms(meta: &fs::Metadata) -> Option<i64> {
    let millis = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(millis).ok()
}

/// 阶段 2 的产物：待写入的单文件索引数据。
struct FileEntry {
    path: String,
    hash: String,
    language: String,
    size: i64,
    mtime: i64,
    symbols: Vec<Symbol>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    /// 建库 + 注册项目，返回 (store, project_id)。
    fn setup(root: &Path) -> (Store, i64) {
        let store = Store::open_in_memory().unwrap();
        let project_id = {
            let conn = store.conn();
            project::insert_project(&conn, &root.to_string_lossy(), "proj").unwrap()
        };
        (store, project_id)
    }

    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// 查某文件的 (name, kind) 符号列表，按名称排序。
    fn symbols_of(store: &Store, project_id: i64, path: &str) -> Vec<(String, String)> {
        let conn = store.conn();
        let mut stmt = conn
            .prepare(
                "SELECT s.name, s.kind FROM symbols s
                 JOIN files f ON s.file_id = f.id
                 WHERE f.project_id = ?1 AND f.path = ?2
                 ORDER BY s.name",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![project_id, path], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows
    }

    fn file_count(store: &Store, project_id: i64) -> i64 {
        let conn = store.conn();
        conn.query_row(
            "SELECT COUNT(*) FROM files WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn index_project_writes_files_and_symbols() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(
            root,
            "src/main.rs",
            "fn alpha() {}\nstruct Beta {\n    x: i32,\n}\n",
        );
        write_file(root, "src/lib.rs", "fn gamma() {}\n");
        write_file(root, "notes.txt", "plain text\n");
        let (store, project_id) = setup(root);

        index_project(&store, project_id, root).unwrap();

        assert_eq!(file_count(&store, project_id), 3);
        let conn = store.conn();
        let record = crate::store::file::get_file(&conn, project_id, "src/main.rs")
            .unwrap()
            .unwrap();
        assert_eq!(record.language.as_deref(), Some("rust"));
        assert_eq!(
            record.size,
            Some("fn alpha() {}\nstruct Beta {\n    x: i32,\n}\n".len() as i64)
        );
        assert_eq!(record.hash.as_deref().map(str::len), Some(64));
        drop(conn);

        assert_eq!(
            symbols_of(&store, project_id, "src/main.rs"),
            vec![
                ("Beta".to_string(), "class".to_string()),
                ("alpha".to_string(), "function".to_string())
            ]
        );
        assert_eq!(
            symbols_of(&store, project_id, "src/lib.rs"),
            vec![("gamma".to_string(), "function".to_string())]
        );
        assert!(
            symbols_of(&store, project_id, "notes.txt").is_empty(),
            "文本文件无符号"
        );

        // 刚索引完不应有失效文件。
        assert!(get_stale_files(&store, project_id).unwrap().is_empty());
    }

    #[test]
    fn index_project_is_idempotent_and_skips_unchanged_files() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "a.rs", "fn one() {}\n");
        write_file(root, "b.rs", "fn two() {}\n");
        let (store, project_id) = setup(root);

        index_project(&store, project_id, root).unwrap();
        {
            let conn = store.conn();
            conn.execute("UPDATE files SET indexed_at = 1", []).unwrap();
        }
        // 第二次全量索引：无变化 → 全部跳过写入。
        index_project(&store, project_id, root).unwrap();
        let conn = store.conn();
        let untouched: i64 = conn
            .query_row("SELECT COUNT(*) FROM files WHERE indexed_at = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(untouched, 2, "未变化文件应被跳过");
        drop(conn);

        // 改动其中一个：只有它被刷新，symbols 同步更新。
        write_file(root, "b.rs", "fn two_prime() {}\n");
        index_project(&store, project_id, root).unwrap();
        let conn = store.conn();
        let (a_at, b_at): (i64, i64) = conn
            .query_row(
                "SELECT MAX(CASE WHEN path = 'a.rs' THEN indexed_at END),
                        MAX(CASE WHEN path = 'b.rs' THEN indexed_at END)
                 FROM files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        drop(conn);
        assert_eq!(a_at, 1, "a.rs 应仍被跳过");
        assert_ne!(b_at, 1, "b.rs 应被重新写入");
        assert_eq!(
            symbols_of(&store, project_id, "b.rs"),
            vec![("two_prime".to_string(), "function".to_string())]
        );
    }

    #[test]
    fn index_project_removes_deleted_files() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "a.rs", "fn keep() {}\n");
        write_file(root, "gone.rs", "fn gone() {}\n");
        let (store, project_id) = setup(root);
        index_project(&store, project_id, root).unwrap();
        assert_eq!(symbols_of(&store, project_id, "gone.rs").len(), 1);

        fs::remove_file(root.join("gone.rs")).unwrap();
        index_project(&store, project_id, root).unwrap();

        assert_eq!(file_count(&store, project_id), 1, "已删除文件应从索引移除");
        let conn = store.conn();
        let record = crate::store::file::get_file(&conn, project_id, "gone.rs").unwrap();
        assert!(record.is_none(), "记录应被删除");
        drop(conn);
        assert!(
            symbols_of(&store, project_id, "gone.rs").is_empty(),
            "symbols 应级联清空"
        );
        assert!(get_stale_files(&store, project_id).unwrap().is_empty());
    }

    #[test]
    fn update_file_index_upserts_single_file() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "src/main.rs", "fn alpha() {}\n");
        write_file(root, "src/other.rs", "fn other() {}\n");
        let (store, project_id) = setup(root);
        index_project(&store, project_id, root).unwrap();

        // 相对路径更新。
        write_file(root, "src/main.rs", "fn alpha() {}\nfn delta() {}\n");
        update_file_index(&store, project_id, Path::new("src/main.rs")).unwrap();
        let names: Vec<String> = symbols_of(&store, project_id, "src/main.rs")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, vec!["alpha".to_string(), "delta".to_string()]);

        // 绝对路径（watcher 事件形态）同样可用。
        write_file(root, "src/main.rs", "fn alpha() {}\n");
        update_file_index(&store, project_id, &root.join("src/main.rs")).unwrap();
        assert_eq!(symbols_of(&store, project_id, "src/main.rs").len(), 1);

        // 内容未变：跳过（indexed_at 不被刷新）。
        {
            let conn = store.conn();
            conn.execute(
                "UPDATE files SET indexed_at = 1 WHERE path = 'src/main.rs'",
                [],
            )
            .unwrap();
        }
        update_file_index(&store, project_id, Path::new("src/main.rs")).unwrap();
        let conn = store.conn();
        let at: i64 = conn
            .query_row(
                "SELECT indexed_at FROM files WHERE path = 'src/main.rs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);
        assert_eq!(at, 1, "未变化文件不应被重复写入");

        // 其他文件不受影响。
        assert_eq!(symbols_of(&store, project_id, "src/other.rs").len(), 1);
    }

    #[test]
    fn remove_file_index_deletes_record_and_symbols() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "src/main.rs", "fn alpha() {}\n");
        write_file(root, "src/lib.rs", "fn gamma() {}\n");
        let (store, project_id) = setup(root);
        index_project(&store, project_id, root).unwrap();

        remove_file_index(&store, project_id, Path::new("src/main.rs")).unwrap();
        assert_eq!(file_count(&store, project_id), 1);
        assert!(
            symbols_of(&store, project_id, "src/main.rs").is_empty(),
            "symbols 应级联删除"
        );
        assert_eq!(symbols_of(&store, project_id, "src/lib.rs").len(), 1);

        // 绝对路径形态同样可用；删除不存在的目标不报错。
        remove_file_index(&store, project_id, &root.join("src/lib.rs")).unwrap();
        remove_file_index(&store, project_id, Path::new("nope.rs")).unwrap();
        assert_eq!(file_count(&store, project_id), 0);
    }

    #[test]
    fn get_stale_files_detects_missing_and_modified() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "a.txt", "one\n");
        write_file(root, "b.txt", "two\n");
        let (store, project_id) = setup(root);
        index_project(&store, project_id, root).unwrap();
        assert!(get_stale_files(&store, project_id).unwrap().is_empty());

        // 确保重写后的 mtime 与索引时不同。
        std::thread::sleep(Duration::from_millis(30));
        write_file(root, "a.txt", "one-changed\n");
        fs::remove_file(root.join("b.txt")).unwrap();

        let mut stale = get_stale_files(&store, project_id).unwrap();
        stale.sort();
        assert_eq!(
            stale,
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
            "修改与删除都应判定为失效"
        );

        // 标准增量流程：存在 → 更新，不存在 → 移除，之后无失效文件。
        for path in stale {
            if root.join(&path).exists() {
                update_file_index(&store, project_id, &path).unwrap();
            } else {
                remove_file_index(&store, project_id, &path).unwrap();
            }
        }
        assert!(get_stale_files(&store, project_id).unwrap().is_empty());
    }

    #[test]
    fn progress_reports_each_file() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "a.txt", "1\n");
        write_file(root, "dir/b.txt", "2\n");
        write_file(root, "dir/c.txt", "3\n");
        let (store, project_id) = setup(root);

        let mut events = Vec::new();
        index_project_with_progress(&store, project_id, root, |p| events.push(p)).unwrap();

        assert_eq!(events.len(), 3);
        assert_eq!(events.last().unwrap().indexed, 3);
        assert!(events.iter().all(|p| p.total == 3));
        let mut paths: Vec<&str> = events.iter().map(|p| p.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["a.txt", "dir/b.txt", "dir/c.txt"]);
    }

    #[test]
    fn unknown_project_returns_config_error() {
        let store = Store::open_in_memory().unwrap();
        let err = update_file_index(&store, 999, Path::new("x.rs")).unwrap_err();
        assert!(matches!(err, XaimError::Config(_)));
        let err = get_stale_files(&store, 999).unwrap_err();
        assert!(matches!(err, XaimError::Config(_)));
    }

    #[test]
    fn indexes_medium_project_under_ten_seconds() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        for d in 0..30u32 {
            for f in 0..20u32 {
                write_file(
                    root,
                    &format!("mod_{d:02}/file_{f:02}.rs"),
                    &format!("fn func_{d}_{f}() {{}}\n"),
                );
            }
        }
        let (store, project_id) = setup(root);

        let start = std::time::Instant::now();
        index_project(&store, project_id, root).unwrap();
        let full = start.elapsed();
        assert!(
            full < Duration::from_secs(10),
            "全量索引耗时 {full:?}，应 < 10s"
        );
        assert_eq!(file_count(&store, project_id), 600);

        // 增量：单文件更新与失效对账都应在 1 秒内。
        let start = std::time::Instant::now();
        std::thread::sleep(Duration::from_millis(30));
        write_file(root, "mod_00/file_00.rs", "fn changed() {}\n");
        update_file_index(&store, project_id, Path::new("mod_00/file_00.rs")).unwrap();
        let incremental = start.elapsed();
        assert!(
            incremental < Duration::from_secs(1),
            "增量索引耗时 {incremental:?}，应 < 1s"
        );

        let start = std::time::Instant::now();
        let stale = get_stale_files(&store, project_id).unwrap();
        let reconcile = start.elapsed();
        assert!(
            reconcile < Duration::from_secs(1),
            "失效对账耗时 {reconcile:?}，应 < 1s"
        );
        assert!(stale.is_empty(), "刚增量更新后不应有失效文件");
    }

    #[tokio::test]
    async fn async_index_matches_sync() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "src/main.rs", "fn alpha() {}\n");
        let (store, project_id) = setup(root);

        index_project_async(store.clone(), project_id, root.to_path_buf())
            .await
            .unwrap();
        assert_eq!(file_count(&store, project_id), 1);

        // 带进度的异步版本同样可用；进度按扫描到的文件数计（含被跳过的）。
        write_file(root, "src/lib.rs", "fn gamma() {}\n");
        let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let received = sink.clone();
        index_project_async_with_progress(
            store.clone(),
            project_id,
            root.to_path_buf(),
            move |p| {
                received.lock().unwrap().push(p);
            },
        )
        .await
        .unwrap();
        let events = sink.lock().unwrap().clone();
        assert_eq!(events.len(), 2, "进度应覆盖全部扫描到的文件");
        assert!(events.iter().all(|p| p.total == 2));
        assert_eq!(file_count(&store, project_id), 2);
    }
}
