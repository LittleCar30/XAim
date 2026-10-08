//! 文件索引持久化：files 表的增删改查（供索引器扫描与刷新）。
//!
//! 约定：函数全部接收 `&Connection` 而不自行加锁，由调用方经
//! `Store::conn()` 独占借用后传入；`(project_id, path)` 唯一；
//! `mtime` 为毫秒级 `i64`，`size` 为字节数；UPDATE/DELETE 类函数
//! 对不存在的目标影响 0 行、不报错。

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::errors::Result;
use crate::types::FileRecord;
use crate::utils::now_ms;

/// files 表查询列清单，列序与 [`row_to_file`] 的读取顺序一致。
const FILE_COLUMNS: &str = "id, project_id, path, language, size, mtime, hash, summary, indexed_at";

/// 插入或刷新文件索引记录（按 `(project_id, path)` 判定），返回记录 id。
///
/// 已存在时原地更新语言/大小/修改时间/哈希并刷新 `indexed_at`；
/// `id` 与已生成的 `summary` 保持不变。特意用 `ON CONFLICT DO UPDATE`
/// 而非 `INSERT OR REPLACE`：后者会删除旧行再插入，换 rowid 的同时
/// 把该文件的 symbols 级联清掉。项目不存在时由外键约束拒绝。
pub fn upsert_file(
    conn: &Connection,
    project_id: i64,
    path: &str,
    language: Option<&str>,
    size: Option<i64>,
    mtime: Option<i64>,
    hash: Option<&str>,
) -> Result<i64> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO files (project_id, path, language, size, mtime, hash, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(project_id, path) DO UPDATE SET
             language = excluded.language,
             size = excluded.size,
             mtime = excluded.mtime,
             hash = excluded.hash,
             indexed_at = excluded.indexed_at",
        params![project_id, path, language, size, mtime, hash, now],
    )?;
    let id: i64 = conn.query_row(
        "SELECT id FROM files WHERE project_id = ?1 AND path = ?2",
        params![project_id, path],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// 按项目与路径查文件记录；不存在返回 `None`。
pub fn get_file(conn: &Connection, project_id: i64, path: &str) -> Result<Option<FileRecord>> {
    Ok(conn
        .query_row(
            &format!("SELECT {FILE_COLUMNS} FROM files WHERE project_id = ?1 AND path = ?2"),
            params![project_id, path],
            row_to_file,
        )
        .optional()?)
}

/// 某项目的全部文件记录，按路径升序。
pub fn list_files(conn: &Connection, project_id: i64) -> Result<Vec<FileRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_COLUMNS} FROM files WHERE project_id = ?1 ORDER BY path ASC"
    ))?;
    let files = stmt
        .query_map([project_id], row_to_file)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(files)
}

/// 删除文件记录；该文件的 symbols 随外键级联删除。
pub fn delete_file(conn: &Connection, project_id: i64, path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM files WHERE project_id = ?1 AND path = ?2",
        params![project_id, path],
    )?;
    Ok(())
}

/// 写入/清空文件摘要；传 `None` 即清空（如文件变更导致旧摘要失效）。
pub fn update_summary(conn: &Connection, file_id: i64, summary: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE files SET summary = ?1 WHERE id = ?2",
        params![summary, file_id],
    )?;
    Ok(())
}

fn row_to_file(row: &Row<'_>) -> rusqlite::Result<FileRecord> {
    Ok(FileRecord {
        id: row.get(0)?,
        project_id: row.get(1)?,
        path: row.get(2)?,
        language: row.get(3)?,
        size: row.get(4)?,
        mtime: row.get(5)?,
        hash: row.get(6)?,
        summary: row.get(7)?,
        indexed_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::project;
    use crate::store::testing::mem_conn;

    fn setup(conn: &Connection) -> i64 {
        project::insert_project(conn, "D:/code/demo", "demo").unwrap()
    }

    #[test]
    fn upsert_file_inserts_new_record() {
        let conn = mem_conn();
        let pid = setup(&conn);

        let id = upsert_file(
            &conn,
            pid,
            "src/main.rs",
            Some("rust"),
            Some(1024),
            Some(1_727_568_000_000),
            Some("blake3:abc"),
        )
        .unwrap();
        assert_eq!(id, 1);

        let record = get_file(&conn, pid, "src/main.rs").unwrap().unwrap();
        assert_eq!(record.id, id);
        assert_eq!(record.project_id, pid);
        assert_eq!(record.path, "src/main.rs");
        assert_eq!(record.language.as_deref(), Some("rust"));
        assert_eq!(record.size, Some(1024));
        assert_eq!(record.mtime, Some(1_727_568_000_000));
        assert_eq!(record.hash.as_deref(), Some("blake3:abc"));
        assert_eq!(record.summary, None, "摘要初始为空");
        assert!(
            record.indexed_at.unwrap() > 1_600_000_000_000,
            "应为毫秒级时间戳"
        );
    }

    #[test]
    fn upsert_file_updates_in_place_keeping_id_symbols_and_summary() {
        let conn = mem_conn();
        let pid = setup(&conn);
        let id = upsert_file(
            &conn,
            pid,
            "src/main.rs",
            Some("rust"),
            Some(100),
            Some(1000),
            Some("hash-1"),
        )
        .unwrap();

        conn.execute(
            "INSERT INTO symbols (file_id, name, kind, start_line, end_line)
             VALUES (?1, 'main', 'function', 1, 3)",
            [id],
        )
        .unwrap();
        update_summary(&conn, id, Some("入口文件")).unwrap();
        // 拨回 indexed_at，验证再次 upsert 会刷新它。
        conn.execute("UPDATE files SET indexed_at = 1000 WHERE id = ?1", [id])
            .unwrap();

        let id2 = upsert_file(
            &conn,
            pid,
            "src/main.rs",
            Some("rust"),
            Some(200),
            Some(2000),
            Some("hash-2"),
        )
        .unwrap();

        assert_eq!(id, id2, "upsert 应原地更新而非换行");
        let record = get_file(&conn, pid, "src/main.rs").unwrap().unwrap();
        assert_eq!(record.hash.as_deref(), Some("hash-2"));
        assert_eq!(record.size, Some(200));
        assert_eq!(record.mtime, Some(2000));
        assert!(record.indexed_at.unwrap() > 1000, "indexed_at 应被刷新");
        assert_eq!(
            record.summary.as_deref(),
            Some("入口文件"),
            "upsert 不应触碰摘要"
        );

        let symbols: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
            .unwrap();
        assert_eq!(symbols, 1, "符号不应被 upsert 级联清除");
    }

    #[test]
    fn upsert_file_rejects_unknown_project_via_foreign_key() {
        let conn = mem_conn();
        let err = upsert_file(&conn, 999, "src/main.rs", None, None, None, None).unwrap_err();
        assert!(
            matches!(err, crate::errors::XaimError::Db(_)),
            "应由外键约束报数据库错误"
        );
    }

    #[test]
    fn get_file_returns_none_for_unknown_project_or_path() {
        let conn = mem_conn();
        let pid = setup(&conn);
        upsert_file(&conn, pid, "src/main.rs", None, None, None, None).unwrap();

        assert!(get_file(&conn, pid, "src/other.rs").unwrap().is_none());
        assert!(get_file(&conn, 999, "src/main.rs").unwrap().is_none());
    }

    #[test]
    fn list_files_sorts_by_path_and_scopes_to_project() {
        let conn = mem_conn();
        let pid = setup(&conn);
        let other = project::insert_project(&conn, "D:/code/other", "other").unwrap();

        upsert_file(&conn, pid, "src/b.rs", None, None, None, None).unwrap();
        upsert_file(&conn, pid, "README.md", None, None, None, None).unwrap();
        upsert_file(&conn, pid, "src/a.rs", None, None, None, None).unwrap();
        upsert_file(&conn, other, "lib.rs", None, None, None, None).unwrap();

        let paths: Vec<String> = list_files(&conn, pid)
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, vec!["README.md", "src/a.rs", "src/b.rs"]);

        let other_paths: Vec<String> = list_files(&conn, other)
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(other_paths, vec!["lib.rs"]);

        // 尚未索引过文件的项目返回空列表。
        let empty = project::insert_project(&conn, "D:/code/empty", "empty").unwrap();
        assert!(list_files(&conn, empty).unwrap().is_empty());
    }

    #[test]
    fn delete_file_removes_row_and_cascades_to_symbols() {
        let conn = mem_conn();
        let pid = setup(&conn);
        let id = upsert_file(&conn, pid, "src/main.rs", None, None, None, None).unwrap();
        conn.execute(
            "INSERT INTO symbols (file_id, name, kind, start_line, end_line)
             VALUES (?1, 'main', 'function', 1, 3)",
            [id],
        )
        .unwrap();

        delete_file(&conn, pid, "src/main.rs").unwrap();

        assert!(get_file(&conn, pid, "src/main.rs").unwrap().is_none());
        let symbols: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
            .unwrap();
        assert_eq!(symbols, 0, "符号应随文件级联删除");

        // 再删一次（已不存在）不报错。
        delete_file(&conn, pid, "src/main.rs").unwrap();
    }

    #[test]
    fn update_summary_sets_clears_and_ignores_unknown_id() {
        let conn = mem_conn();
        let pid = setup(&conn);
        let id = upsert_file(&conn, pid, "src/main.rs", None, None, None, None).unwrap();

        update_summary(&conn, id, Some("程序入口")).unwrap();
        assert_eq!(
            get_file(&conn, pid, "src/main.rs")
                .unwrap()
                .unwrap()
                .summary
                .as_deref(),
            Some("程序入口")
        );

        update_summary(&conn, id, None).unwrap();
        assert_eq!(
            get_file(&conn, pid, "src/main.rs")
                .unwrap()
                .unwrap()
                .summary,
            None
        );

        // 不存在的文件 id 影响 0 行、不报错。
        update_summary(&conn, 999, Some("孤儿摘要")).unwrap();
    }
}
