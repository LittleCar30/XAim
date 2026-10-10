//! SQLite schema：全部建表 SQL、索引与版本化迁移。
//!
//! 约定：
//! - 所有时间戳列（`created_at` / `last_opened_at` / `mtime` / `indexed_at` /
//!   `finished_at` / `applied_at` / `expire_at`）一律为毫秒级 `i64`；
//! - 子表外键一律 `ON DELETE CASCADE`，删除父记录时由 SQLite 级联清理子记录
//!   （要求连接开启 `PRAGMA foreign_keys = ON`，[`migrate`] 会自行开启）；
//! - 迁移按版本递增执行：[`MIGRATIONS`][MIGRATIONS] 的第 i 项把 schema 从版本 i
//!   升级到版本 i+1，每个版本在单个事务内应用并写入 `schema_version`。

use rusqlite::{params, Connection};

use crate::errors::{Result, XaimError};
use crate::utils::now_ms;

/// 当前 schema 的最新版本。
pub const CURRENT_SCHEMA_VERSION: i64 = 3;

/// 版本登记表。必须先于任何迁移存在，用于记录/查询已应用的版本；
/// 行数极少，`version` 为主键自带索引，`applied_at` 仅供审计。
const CREATE_SCHEMA_VERSION: &str = "
    CREATE TABLE IF NOT EXISTS schema_version (
        version    INTEGER PRIMARY KEY,
        applied_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_schema_version_applied ON schema_version(applied_at);
";

/// 版本 0 -> 1：初始建表与索引。
const V1: &[&str] = &[
    // 项目：path 唯一，保证重复打开同一目录不会产生重复记录。
    "CREATE TABLE IF NOT EXISTS projects (
        id             INTEGER PRIMARY KEY AUTOINCREMENT,
        path           TEXT UNIQUE NOT NULL,
        name           TEXT NOT NULL,
        created_at     INTEGER NOT NULL,
        last_opened_at INTEGER NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_projects_last_opened ON projects(last_opened_at)",
    // 文件索引快照：同一项目内 path 唯一。
    "CREATE TABLE IF NOT EXISTS files (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id INTEGER NOT NULL,
        path       TEXT NOT NULL,
        language   TEXT,
        size       INTEGER,
        mtime      INTEGER,
        hash       TEXT,
        summary    TEXT,
        indexed_at INTEGER,
        UNIQUE(project_id, path),
        FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS idx_files_project ON files(project_id)",
    "CREATE INDEX IF NOT EXISTS idx_files_hash ON files(hash)",
    // 代码符号。
    "CREATE TABLE IF NOT EXISTS symbols (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        file_id    INTEGER NOT NULL,
        name       TEXT NOT NULL,
        kind       TEXT NOT NULL,
        start_line INTEGER NOT NULL,
        end_line   INTEGER NOT NULL,
        FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name)",
    "CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id)",
    // 编码任务。
    "CREATE TABLE IF NOT EXISTS tasks (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        project_id   INTEGER NOT NULL,
        prompt       TEXT NOT NULL,
        status       TEXT NOT NULL,
        intent       TEXT,
        plan         TEXT,
        created_at   INTEGER NOT NULL,
        finished_at  INTEGER,
        duration_ms  INTEGER,
        token_input  INTEGER,
        token_output INTEGER,
        cost         REAL,
        FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project_id)",
    "CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status)",
    // 任务执行流水中的单步记录。
    "CREATE TABLE IF NOT EXISTS task_steps (
        id        INTEGER PRIMARY KEY AUTOINCREMENT,
        task_id   INTEGER NOT NULL,
        step_type TEXT NOT NULL,
        content   TEXT,
        created_at INTEGER NOT NULL,
        FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS idx_steps_task ON task_steps(task_id)",
    // 候选补丁；applied 为 0/1。
    "CREATE TABLE IF NOT EXISTS patches (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        task_id    INTEGER NOT NULL,
        file_path  TEXT NOT NULL,
        diff       TEXT NOT NULL,
        applied    INTEGER NOT NULL DEFAULT 0,
        score      REAL,
        created_at INTEGER NOT NULL,
        FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS idx_patches_task ON patches(task_id)",
    // 通用 KV 缓存；expire_at 为 NULL 表示永不过期。
    "CREATE TABLE IF NOT EXISTS cache (
        key       TEXT PRIMARY KEY,
        value     TEXT NOT NULL,
        expire_at INTEGER
    )",
    "CREATE INDEX IF NOT EXISTS idx_cache_expire ON cache(expire_at)",
];

/// 版本 1 -> 2：为符号检索补 NOCASE 名字索引。
///
/// SQLite 的 LIKE 前缀优化在默认 `case_sensitive_like = OFF` 下只作用于
/// NOCASE 排序的索引；V1 的 `idx_symbols_name` 是 BINARY 排序，服务不了
/// `name LIKE 'q%'`（符号检索的前缀档），故补一个 NOCASE 索引。
const V2: &[&str] =
    &["CREATE INDEX IF NOT EXISTS idx_symbols_name_nocase ON symbols(name COLLATE NOCASE)"];

/// 全部迁移步骤：`MIGRATIONS[i]` 把 schema 从版本 i 升级到版本 i + 1。
/// 新增迁移时在数组末尾追加一项，并把 [`CURRENT_SCHEMA_VERSION`] 加一。
const V3: &[&str] = &[
    "ALTER TABLE tasks ADD COLUMN conversation_id INTEGER REFERENCES tasks(id)",
    "ALTER TABLE tasks ADD COLUMN parent_task_id INTEGER REFERENCES tasks(id)",
    "UPDATE tasks SET conversation_id = id",
    "CREATE INDEX idx_tasks_conversation ON tasks(conversation_id, id)",
];
const MIGRATIONS: &[&[&str]] = &[V1, V2, V3];

/// 把数据库 schema 迁移到 [`CURRENT_SCHEMA_VERSION`]。
///
/// 幂等：已应用的版本自动跳过，重复调用不产生任何变更也不报错。
/// 若库中记录的版本比程序支持的还新（用户把新版数据拷给了旧程序），
/// 返回 [`XaimError::Config`] 拒绝降级，避免旧代码写坏新 schema。
pub fn migrate(conn: &Connection) -> Result<()> {
    // 迁移的 DDL 依赖级联行为（如重建父表），必须先确保外键开启。
    // 注意该 PRAGMA 在事务内是空操作，故放在所有事务之外。
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(CREATE_SCHEMA_VERSION)?;

    let current = applied_version(conn)?;
    if current > CURRENT_SCHEMA_VERSION {
        return Err(XaimError::Config(format!(
            "数据库 schema 版本 {current} 高于程序支持的版本 {CURRENT_SCHEMA_VERSION}，请升级应用"
        )));
    }

    for (offset, statements) in MIGRATIONS.iter().enumerate() {
        let target = offset as i64 + 1;
        if target <= current {
            continue;
        }
        apply_migration(conn, target, statements)?;
    }
    Ok(())
}

/// 查询已应用的最高 schema 版本；全新库返回 0。
pub fn applied_version(conn: &Connection) -> Result<i64> {
    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )?;
    Ok(version)
}

/// 在单个事务内应用一个版本的迁移并登记版本号：
/// 要么全部生效并写入 `schema_version`，要么整体回滚。
fn apply_migration(conn: &Connection, target: i64, statements: &[&str]) -> Result<()> {
    // migrate 拿到的是 &Connection，故用 unchecked_transaction 开事务。
    let tx = conn.unchecked_transaction()?;
    for sql in statements {
        tx.execute_batch(sql)?;
    }
    tx.execute(
        "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
        params![target, now_ms()],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 迁移后应存在的全部表。
    const ALL_TABLES: &[&str] = &[
        "projects",
        "files",
        "symbols",
        "tasks",
        "task_steps",
        "patches",
        "cache",
        "schema_version",
    ];

    fn mem_conn() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn migrate_creates_all_tables() {
        let conn = mem_conn();
        migrate(&conn).unwrap();
        for table in ALL_TABLES {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "表 {table} 应存在");
        }
    }

    #[test]
    fn migrate_twice_without_error() {
        // 第二次运行不报错，也不重复登记版本（每个版本恰好一行）。
        let conn = mem_conn();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(count(&conn, "schema_version"), CURRENT_SCHEMA_VERSION);
        assert_eq!(applied_version(&conn).unwrap(), CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn every_table_has_at_least_one_index() {
        let conn = mem_conn();
        migrate(&conn).unwrap();
        for table in ALL_TABLES {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'index' AND tbl_name = ?1 AND name NOT LIKE 'sqlite_%'",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(n >= 1, "表 {table} 应至少有一个显式索引");
        }
    }

    #[test]
    fn migrate_records_version_with_timestamp() {
        let conn = mem_conn();
        migrate(&conn).unwrap();
        let (version, applied_at): (i64, i64) = conn
            .query_row(
                "SELECT version, applied_at FROM schema_version
                 WHERE version = (SELECT MAX(version) FROM schema_version)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        assert!(applied_at > 0, "applied_at 应为毫秒时间戳");
    }

    #[test]
    fn delete_project_cascades_to_all_children() {
        let conn = mem_conn();
        migrate(&conn).unwrap();

        conn.execute_batch(
            "INSERT INTO projects (id, path, name, created_at, last_opened_at)
                 VALUES (1, 'D:/code/demo', 'demo', 0, 0);
             INSERT INTO files (id, project_id, path)
                 VALUES (1, 1, 'src/main.rs');
             INSERT INTO symbols (id, file_id, name, kind, start_line, end_line)
                 VALUES (1, 1, 'main', 'function', 1, 3);
             INSERT INTO tasks (id, project_id, prompt, status, created_at)
                 VALUES (1, 1, 'add login', 'idle', 0);
             INSERT INTO task_steps (id, task_id, step_type, created_at)
                 VALUES (1, 1, 'plan', 0);
             INSERT INTO patches (id, task_id, file_path, diff, created_at)
                 VALUES (1, 1, 'src/main.rs', '--- a', 0);
             INSERT INTO cache (key, value) VALUES ('k', 'v');",
        )
        .unwrap();

        conn.execute("DELETE FROM projects WHERE id = 1", [])
            .unwrap();

        // 无外键的表不受影响。
        assert_eq!(count(&conn, "cache"), 1);
        for table in ["files", "symbols", "tasks", "task_steps", "patches"] {
            assert_eq!(count(&conn, table), 0, "删除项目后 {table} 应被级联清空");
        }
    }

    #[test]
    fn rejects_database_from_newer_app_version() {
        let conn = mem_conn();
        migrate(&conn).unwrap();
        conn.execute(
            "UPDATE schema_version SET version = ?1
             WHERE version = (SELECT MAX(version) FROM schema_version)",
            [CURRENT_SCHEMA_VERSION + 1],
        )
        .unwrap();
        let err = migrate(&conn).unwrap_err();
        assert!(matches!(err, XaimError::Config(_)));
        assert!(err.to_string().contains("高于"));
    }

    #[test]
    fn v2_tasks_migrate_to_independent_conversations() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(CREATE_SCHEMA_VERSION).unwrap();
        for statement in V1.iter().chain(V2.iter()) {
            conn.execute_batch(statement).unwrap();
        }
        conn.execute_batch("INSERT INTO schema_version VALUES (1, 0), (2, 0);
            INSERT INTO projects VALUES (1, 'project', 'demo', 0, 0);
            INSERT INTO tasks (id, project_id, prompt, status, created_at) VALUES (1, 1, 'old', 'done', 0);").unwrap();
        migrate(&conn).unwrap();
        let task = crate::store::task::get_task(&conn, 1).unwrap().unwrap();
        assert_eq!(task.conversation_id, Some(task.id));
        assert_eq!(task.parent_task_id, None);
        migrate(&conn).unwrap();
    }
}
