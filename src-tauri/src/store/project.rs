//! 项目持久化：projects 表的增删改查。
//!
//! 约定：函数全部接收 `&Connection` 而不自行加锁，由调用方
//! （命令层 / `Store`）经 `Store::conn()` 独占借用后传入；
//! `path` 为唯一键，比较区分大小写（SQLite 默认 BINARY 排序）。

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::errors::Result;
use crate::types::Project;
use crate::utils::now_ms;

/// 各查询共用的列清单，避免重复书写导致列序漂移。
const COLUMNS: &str = "id, path, name, created_at, last_opened_at";

/// 插入项目并返回其 id；`created_at` 与 `last_opened_at` 均为当前毫秒时间戳。
///
/// `path` 唯一：已存在同路径项目时 `INSERT OR IGNORE` 跳过插入，
/// 返回既有记录的 id——语义为"打开即注册"，重复打开同一目录幂等。
pub fn insert_project(conn: &Connection, path: &str, name: &str) -> Result<i64> {
    let now = now_ms();
    conn.execute(
        "INSERT OR IGNORE INTO projects (path, name, created_at, last_opened_at)
         VALUES (?1, ?2, ?3, ?3)",
        params![path, name, now],
    )?;
    let id: i64 = conn.query_row("SELECT id FROM projects WHERE path = ?1", [path], |row| {
        row.get(0)
    })?;
    Ok(id)
}

/// 按主键查项目；不存在返回 `None`。
pub fn get_project(conn: &Connection, id: i64) -> Result<Option<Project>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM projects WHERE id = ?1"),
            [id],
            row_to_project,
        )
        .optional()?)
}

/// 按项目路径查项目；不存在返回 `None`。
pub fn get_project_by_path(conn: &Connection, path: &str) -> Result<Option<Project>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM projects WHERE path = ?1"),
            [path],
            row_to_project,
        )
        .optional()?)
}

/// 全部项目，按最近打开时间倒序（配合 idx_projects_last_opened 索引）。
pub fn list_projects(conn: &Connection) -> Result<Vec<Project>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM projects ORDER BY last_opened_at DESC, id DESC"
    ))?;
    let projects = stmt
        .query_map([], row_to_project)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(projects)
}

/// 刷新项目最近打开时间为当前毫秒时间戳。
pub fn update_last_opened(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE projects SET last_opened_at = ?1 WHERE id = ?2",
        params![now_ms(), id],
    )?;
    Ok(())
}

/// 删除项目；其文件/符号/任务/步骤/补丁随外键 `ON DELETE CASCADE` 一并删除。
pub fn delete_project(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM projects WHERE id = ?1", [id])?;
    Ok(())
}

fn row_to_project(row: &Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        path: row.get(1)?,
        name: row.get(2)?,
        created_at: row.get(3)?,
        last_opened_at: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::mem_conn;

    #[test]
    fn insert_project_returns_new_id_with_millisecond_timestamps() {
        let conn = mem_conn();
        let id = insert_project(&conn, "D:/code/demo", "demo").unwrap();
        assert_eq!(id, 1);

        let project = get_project(&conn, id).unwrap().unwrap();
        assert_eq!(project.path, "D:/code/demo");
        assert_eq!(project.name, "demo");
        assert!(project.created_at > 1_600_000_000_000, "应为毫秒级时间戳");
        assert_eq!(project.last_opened_at, project.created_at);
    }

    #[test]
    fn insert_project_is_idempotent_for_duplicate_path() {
        let conn = mem_conn();
        let first = insert_project(&conn, "D:/code/demo", "demo").unwrap();
        let second = insert_project(&conn, "D:/code/demo", "renamed").unwrap();

        assert_eq!(first, second, "重复路径应返回既有 id");
        assert_eq!(list_projects(&conn).unwrap().len(), 1, "不应产生重复行");
        // OR IGNORE 不更新已有行：名称保持首次插入的值。
        let project = get_project(&conn, first).unwrap().unwrap();
        assert_eq!(project.name, "demo");
    }

    #[test]
    fn get_project_returns_none_for_unknown_id() {
        let conn = mem_conn();
        assert!(get_project(&conn, 42).unwrap().is_none());
    }

    #[test]
    fn get_project_by_path_matches_exactly() {
        let conn = mem_conn();
        let id = insert_project(&conn, "D:/code/demo", "demo").unwrap();

        let found = get_project_by_path(&conn, "D:/code/demo").unwrap().unwrap();
        assert_eq!(found.id, id);
        assert!(get_project_by_path(&conn, "D:/code/other")
            .unwrap()
            .is_none());
    }

    #[test]
    fn list_projects_is_empty_then_orders_by_last_opened_desc() {
        let conn = mem_conn();
        assert!(list_projects(&conn).unwrap().is_empty());

        let a = insert_project(&conn, "D:/a", "a").unwrap();
        let b = insert_project(&conn, "D:/b", "b").unwrap();
        let c = insert_project(&conn, "D:/c", "c").unwrap();
        // 人为拉开 last_opened_at，验证倒序。
        conn.execute(
            "UPDATE projects SET last_opened_at = 100 WHERE id = ?1",
            [a],
        )
        .unwrap();
        conn.execute(
            "UPDATE projects SET last_opened_at = 300 WHERE id = ?1",
            [b],
        )
        .unwrap();
        conn.execute(
            "UPDATE projects SET last_opened_at = 200 WHERE id = ?1",
            [c],
        )
        .unwrap();

        let ids: Vec<i64> = list_projects(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec![b, c, a]);
    }

    #[test]
    fn update_last_opened_only_touches_target_row() {
        let conn = mem_conn();
        let a = insert_project(&conn, "D:/a", "a").unwrap();
        let b = insert_project(&conn, "D:/b", "b").unwrap();
        conn.execute(
            "UPDATE projects SET last_opened_at = 1000 WHERE id IN (?1, ?2)",
            params![a, b],
        )
        .unwrap();

        update_last_opened(&conn, a).unwrap();

        let refreshed = get_project(&conn, a).unwrap().unwrap();
        assert!(refreshed.last_opened_at > 1000, "应刷新为当前毫秒时间戳");
        let untouched = get_project(&conn, b).unwrap().unwrap();
        assert_eq!(untouched.last_opened_at, 1000, "其他行不应被修改");
    }

    #[test]
    fn delete_project_removes_row_and_cascades_to_tasks() {
        let conn = mem_conn();
        let id = insert_project(&conn, "D:/code/demo", "demo").unwrap();
        conn.execute(
            "INSERT INTO tasks (project_id, prompt, status, created_at)
             VALUES (?1, 'add login', 'idle', 0)",
            [id],
        )
        .unwrap();

        delete_project(&conn, id).unwrap();

        assert!(get_project(&conn, id).unwrap().is_none());
        let tasks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tasks, 0, "任务应随项目级联删除");
    }

    #[test]
    fn delete_project_with_unknown_id_is_ok() {
        let conn = mem_conn();
        delete_project(&conn, 999).unwrap();
    }
}
