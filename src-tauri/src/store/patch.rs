//! 补丁持久化：patches 表的增删改查。
//!
//! 约定：函数全部接收 `&Connection` 而不自行加锁，由调用方经
//! `Store::conn()` 独占借用后传入；`diff` 为 TEXT、`score` 为 REAL、
//! `applied` 为 0/1 整数（领域类型映射为 `bool`）；时间戳为毫秒级 `i64`；
//! UPDATE 类函数对不存在的 id 影响 0 行、不报错。

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::errors::Result;
use crate::types::Patch;
use crate::utils::now_ms;

/// patches 表查询列清单，列序与 [`row_to_patch`] 的读取顺序一致。
const PATCH_COLUMNS: &str = "id, task_id, file_path, diff, applied, score, created_at";

/// 记录一份候选补丁（`applied` 初始为 false），返回新补丁 id。
///
/// `score` 可为 `None`（尚未评分）；任务不存在时由外键约束拒绝。
pub fn insert_patch(
    conn: &Connection,
    task_id: i64,
    file_path: &str,
    diff: &str,
    score: Option<f64>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO patches (task_id, file_path, diff, applied, score, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![task_id, file_path, diff, false, score, now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 按主键查补丁；不存在返回 `None`。
pub fn get_patch(conn: &Connection, id: i64) -> Result<Option<Patch>> {
    Ok(conn
        .query_row(
            &format!("SELECT {PATCH_COLUMNS} FROM patches WHERE id = ?1"),
            [id],
            row_to_patch,
        )
        .optional()?)
}

/// 某任务的补丁列表，按创建时间倒序（最新候选在前），同毫秒按 id 兜底。
pub fn list_patches_by_task(conn: &Connection, task_id: i64) -> Result<Vec<Patch>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PATCH_COLUMNS} FROM patches
         WHERE task_id = ?1
         ORDER BY created_at DESC, id DESC"
    ))?;
    let patches = stmt
        .query_map([task_id], row_to_patch)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(patches)
}

/// 把补丁标记为已应用。
pub fn mark_applied(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE patches SET applied = ?1 WHERE id = ?2",
        params![true, id],
    )?;
    Ok(())
}

/// 更新补丁评分；传 `None` 即清空评分。
pub fn update_score(conn: &Connection, id: i64, score: Option<f64>) -> Result<()> {
    conn.execute(
        "UPDATE patches SET score = ?1 WHERE id = ?2",
        params![score, id],
    )?;
    Ok(())
}

fn row_to_patch(row: &Row<'_>) -> rusqlite::Result<Patch> {
    Ok(Patch {
        id: row.get(0)?,
        task_id: row.get(1)?,
        file_path: row.get(2)?,
        diff: row.get(3)?,
        applied: row.get(4)?,
        score: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::project;
    use crate::store::task;
    use crate::store::testing::mem_conn;

    /// 建项目 + 任务，返回 (project_id, task_id)。
    fn setup(conn: &Connection) -> (i64, i64) {
        let pid = project::insert_project(conn, "D:/code/demo", "demo").unwrap();
        let tid = task::insert_task(conn, pid, "add login").unwrap();
        (pid, tid)
    }

    #[test]
    fn insert_patch_creates_unapplied_patch_with_timestamps() {
        let conn = mem_conn();
        let (_, tid) = setup(&conn);
        let diff = "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,4 @@\n fn main() {";

        let id = insert_patch(&conn, tid, "src/main.rs", diff, Some(0.87)).unwrap();

        let patch = get_patch(&conn, id).unwrap().unwrap();
        assert_eq!(patch.task_id, tid);
        assert_eq!(patch.file_path, "src/main.rs");
        assert_eq!(patch.diff, diff, "多行 diff 应原样存取");
        assert!(!patch.applied, "新补丁初始未应用");
        assert_eq!(patch.score, Some(0.87));
        assert!(patch.created_at > 1_600_000_000_000, "应为毫秒级时间戳");
    }

    #[test]
    fn insert_patch_keeps_null_score() {
        let conn = mem_conn();
        let (_, tid) = setup(&conn);

        let id = insert_patch(&conn, tid, "src/lib.rs", "--- a\n+++ b", None).unwrap();
        let patch = get_patch(&conn, id).unwrap().unwrap();
        assert_eq!(patch.score, None);
    }

    #[test]
    fn insert_patch_rejects_unknown_task_via_foreign_key() {
        let conn = mem_conn();
        let err = insert_patch(&conn, 999, "src/main.rs", "--- a", None).unwrap_err();
        assert!(
            matches!(err, crate::errors::XaimError::Db(_)),
            "应由外键约束报数据库错误"
        );
    }

    #[test]
    fn get_patch_returns_none_for_unknown_id() {
        let conn = mem_conn();
        assert!(get_patch(&conn, 42).unwrap().is_none());
    }

    #[test]
    fn list_patches_by_task_returns_newest_first_for_that_task_only() {
        let conn = mem_conn();
        let (pid, tid) = setup(&conn);
        let other = task::insert_task(&conn, pid, "unrelated").unwrap();
        let fresh = task::insert_task(&conn, pid, "no patches").unwrap();

        let p1 = insert_patch(&conn, tid, "a.rs", "--- a1", None).unwrap();
        let p2 = insert_patch(&conn, tid, "b.rs", "--- b1", None).unwrap();
        let p3 = insert_patch(&conn, tid, "c.rs", "--- c1", None).unwrap();
        insert_patch(&conn, other, "d.rs", "--- d1", None).unwrap();

        // 最新候选在前；同一毫秒内按 id 倒序兜底。
        let ids: Vec<i64> = list_patches_by_task(&conn, tid)
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec![p3, p2, p1]);

        let other_ids: Vec<i64> = list_patches_by_task(&conn, other)
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(other_ids.len(), 1, "只应包含该任务自己的补丁");

        assert!(list_patches_by_task(&conn, fresh).unwrap().is_empty());
    }

    #[test]
    fn mark_applied_flips_flag_only_for_target() {
        let conn = mem_conn();
        let (_, tid) = setup(&conn);
        let p1 = insert_patch(&conn, tid, "a.rs", "--- a", None).unwrap();
        let p2 = insert_patch(&conn, tid, "b.rs", "--- b", None).unwrap();

        mark_applied(&conn, p1).unwrap();

        assert!(get_patch(&conn, p1).unwrap().unwrap().applied);
        assert!(!get_patch(&conn, p2).unwrap().unwrap().applied);

        // 对不存在的补丁影响 0 行、不报错。
        mark_applied(&conn, 999).unwrap();
    }

    #[test]
    fn update_score_sets_overwrites_and_clears() {
        let conn = mem_conn();
        let (_, tid) = setup(&conn);
        let id = insert_patch(&conn, tid, "a.rs", "--- a", None).unwrap();

        update_score(&conn, id, Some(0.5)).unwrap();
        assert_eq!(get_patch(&conn, id).unwrap().unwrap().score, Some(0.5));

        update_score(&conn, id, Some(0.99)).unwrap();
        assert_eq!(get_patch(&conn, id).unwrap().unwrap().score, Some(0.99));

        update_score(&conn, id, None).unwrap();
        assert_eq!(get_patch(&conn, id).unwrap().unwrap().score, None);
    }
}
