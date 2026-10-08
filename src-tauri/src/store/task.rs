//! 任务持久化：tasks / task_steps 两张表的增删改查。
//!
//! 约定：
//! - 函数全部接收 `&Connection` 而不自行加锁，由调用方经
//!   `Store::conn()` 独占借用后传入；
//! - `status` 以字符串形式存储，序列化规则沿用 [`TaskStatus`] 的
//!   serde camelCase 命名（与前端 JSON 契约同一来源），如 "applyingPatch"；
//! - 时间戳为毫秒级 `i64`；
//! - UPDATE 类函数对不存在的 id 影响 0 行、不报错。

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::errors::{Result, XaimError};
use crate::types::{Task, TaskStatus, TaskStep};
use crate::utils::now_ms;

/// tasks 表查询列清单，列序与 [`row_to_task`] 的读取顺序一致。
const TASK_COLUMNS: &str =
    "id, project_id, prompt, status, intent, plan, created_at, finished_at, duration_ms, token_input, token_output, cost";

/// task_steps 表查询列清单，列序与 [`row_to_step`] 的读取顺序一致。
const STEP_COLUMNS: &str = "id, task_id, step_type, content, created_at";

/// 创建任务（初始状态 `idle`），返回新任务 id。
///
/// 项目不存在时由外键约束拒绝（`XaimError::Db`）。
pub fn insert_task(conn: &Connection, project_id: i64, prompt: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO tasks (project_id, prompt, status, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            project_id,
            prompt,
            status_to_str(TaskStatus::Idle),
            now_ms()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 按主键查任务；不存在返回 `None`。
pub fn get_task(conn: &Connection, id: i64) -> Result<Option<Task>> {
    Ok(conn
        .query_row(
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
            [id],
            row_to_task,
        )
        .optional()?)
}

/// 某项目的任务列表，按创建时间倒序（最新在前），同毫秒按 id 兜底；
/// `limit` 限制条数（0 返回空列表）。
pub fn list_tasks(conn: &Connection, project_id: i64, limit: usize) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {TASK_COLUMNS} FROM tasks
         WHERE project_id = ?1
         ORDER BY created_at DESC, id DESC
         LIMIT ?2"
    ))?;
    let tasks = stmt
        .query_map(params![project_id, limit as i64], row_to_task)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tasks)
}

/// 更新任务状态；不触碰 `finished_at` / `duration_ms`（见 [`finish_task`]）。
pub fn update_task_status(conn: &Connection, id: i64, status: TaskStatus) -> Result<()> {
    conn.execute(
        "UPDATE tasks SET status = ?1 WHERE id = ?2",
        params![status_to_str(status), id],
    )?;
    Ok(())
}

/// 记录意图解析结果。
pub fn update_task_intent(conn: &Connection, id: i64, intent: &str) -> Result<()> {
    conn.execute(
        "UPDATE tasks SET intent = ?1 WHERE id = ?2",
        params![intent, id],
    )?;
    Ok(())
}

/// 记录执行计划。
pub fn update_task_plan(conn: &Connection, id: i64, plan: &str) -> Result<()> {
    conn.execute(
        "UPDATE tasks SET plan = ?1 WHERE id = ?2",
        params![plan, id],
    )?;
    Ok(())
}

/// 写入任务统计；四个字段整体覆盖，传 `None` 即清空对应列。
pub fn update_task_stats(
    conn: &Connection,
    id: i64,
    duration_ms: Option<i64>,
    token_input: Option<i64>,
    token_output: Option<i64>,
    cost: Option<f64>,
) -> Result<()> {
    conn.execute(
        "UPDATE tasks SET duration_ms = ?1, token_input = ?2, token_output = ?3, cost = ?4
         WHERE id = ?5",
        params![duration_ms, token_input, token_output, cost, id],
    )?;
    Ok(())
}

/// 结束任务：写入终态 `status` 与 `finished_at`，
/// 并按 `finished_at - created_at` 推算 `duration_ms`。
pub fn finish_task(conn: &Connection, id: i64, status: TaskStatus) -> Result<()> {
    conn.execute(
        "UPDATE tasks
         SET status = ?1,
             finished_at = ?2,
             duration_ms = ?2 - created_at
         WHERE id = ?3",
        params![status_to_str(status), now_ms(), id],
    )?;
    Ok(())
}

/// 追加一条任务流水记录，返回新记录 id。
///
/// 任务不存在时由外键约束拒绝（`XaimError::Db`）。
pub fn insert_step(conn: &Connection, task_id: i64, step_type: &str, content: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO task_steps (task_id, step_type, content, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![task_id, step_type, content, now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 某任务的流水记录，按时间正序（即执行顺序），同毫秒按 id 兜底。
pub fn list_steps(conn: &Connection, task_id: i64) -> Result<Vec<TaskStep>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {STEP_COLUMNS} FROM task_steps WHERE task_id = ?1 ORDER BY created_at ASC, id ASC"
    ))?;
    let steps = stmt
        .query_map([task_id], row_to_step)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(steps)
}

fn row_to_task(row: &Row<'_>) -> rusqlite::Result<Task> {
    let status: String = row.get(3)?;
    Ok(Task {
        id: row.get(0)?,
        project_id: row.get(1)?,
        prompt: row.get(2)?,
        status: status_from_text(&status)?,
        intent: row.get(4)?,
        plan: row.get(5)?,
        created_at: row.get(6)?,
        finished_at: row.get(7)?,
        duration_ms: row.get(8)?,
        token_input: row.get(9)?,
        token_output: row.get(10)?,
        cost: row.get(11)?,
    })
}

fn row_to_step(row: &Row<'_>) -> rusqlite::Result<TaskStep> {
    Ok(TaskStep {
        id: row.get(0)?,
        task_id: row.get(1)?,
        step_type: row.get(2)?,
        // 列在库里可空，但领域类型要求非空：历史 NULL 视为空串。
        content: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        created_at: row.get(4)?,
    })
}

/// `TaskStatus` -> 存储字符串。沿用 types.rs 的 serde camelCase 命名，
/// 与前端 JSON 契约保持单一来源，避免两份映射表漂移。
fn status_to_str(status: TaskStatus) -> String {
    serde_json::to_value(status)
        .expect("TaskStatus 序列化不会失败")
        .as_str()
        .expect("无字段枚举应序列化为字符串")
        .to_string()
}

/// 存储字符串 -> `TaskStatus`；未知值视为数据损坏，返回内部错误。
fn str_to_status(text: &str) -> Result<TaskStatus> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|_| XaimError::Unknown(format!("数据库中存在未知的任务状态: {text}")))
}

/// 供 query_row / query_map 使用的行内转换：把未知状态包成 rusqlite 错误。
fn status_from_text(text: &str) -> rusqlite::Result<TaskStatus> {
    str_to_status(text).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::project;
    use crate::store::testing::mem_conn;

    /// 建项目 + 任务，返回 (project_id, task_id)。
    fn setup(conn: &Connection, project_path: &str) -> (i64, i64) {
        let pid = project::insert_project(conn, project_path, "demo").unwrap();
        let tid = insert_task(conn, pid, "add login").unwrap();
        (pid, tid)
    }

    #[test]
    fn insert_task_creates_idle_task_with_timestamps() {
        let conn = mem_conn();
        let (pid, tid) = setup(&conn, "D:/code/demo");

        let task = get_task(&conn, tid).unwrap().unwrap();
        assert_eq!(task.id, tid);
        assert_eq!(task.project_id, pid);
        assert_eq!(task.prompt, "add login");
        assert_eq!(task.status, TaskStatus::Idle);
        assert!(task.created_at > 1_600_000_000_000, "应为毫秒级时间戳");
        assert_eq!(task.intent, None);
        assert_eq!(task.plan, None);
        assert_eq!(task.finished_at, None);
        assert_eq!(task.duration_ms, None);
        assert_eq!(task.token_input, None);
        assert_eq!(task.token_output, None);
        assert_eq!(task.cost, None);
    }

    #[test]
    fn insert_task_rejects_unknown_project_via_foreign_key() {
        let conn = mem_conn();
        let err = insert_task(&conn, 999, "orphan").unwrap_err();
        assert!(matches!(err, XaimError::Db(_)), "应由外键约束报数据库错误");
    }

    #[test]
    fn get_task_returns_none_for_unknown_id() {
        let conn = mem_conn();
        assert!(get_task(&conn, 42).unwrap().is_none());
    }

    #[test]
    fn list_tasks_filters_by_project_and_limits_newest_first() {
        let conn = mem_conn();
        let pid = project::insert_project(&conn, "D:/code/demo", "demo").unwrap();
        let other = project::insert_project(&conn, "D:/code/other", "other").unwrap();
        let t1 = insert_task(&conn, pid, "first").unwrap();
        let t2 = insert_task(&conn, pid, "second").unwrap();
        let t3 = insert_task(&conn, pid, "third").unwrap();
        insert_task(&conn, other, "unrelated").unwrap();

        // 最新在前；同一毫秒内按 id 倒序兜底。
        let ids: Vec<i64> = list_tasks(&conn, pid, 10)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![t3, t2, t1]);

        let limited: Vec<i64> = list_tasks(&conn, pid, 2)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(limited, vec![t3, t2]);

        assert!(
            list_tasks(&conn, pid, 0).unwrap().is_empty(),
            "limit 0 应返回空"
        );
    }

    #[test]
    fn update_task_status_roundtrips_every_variant() {
        // 覆盖全部 15 个状态，即所有状态转换的目标值都能写库并读回。
        let all = [
            TaskStatus::Idle,
            TaskStatus::Parsing,
            TaskStatus::Indexing,
            TaskStatus::Retrieving,
            TaskStatus::Planning,
            TaskStatus::Generating,
            TaskStatus::ApplyingPatch,
            TaskStatus::Verifying,
            TaskStatus::Repairing,
            TaskStatus::WaitingConfirm,
            TaskStatus::Applied,
            TaskStatus::Done,
            TaskStatus::Failed,
            TaskStatus::RolledBack,
            TaskStatus::Cancelled,
        ];
        let conn = mem_conn();
        let (_, tid) = setup(&conn, "D:/code/demo");

        for expected in all {
            update_task_status(&conn, tid, expected).unwrap();
            let stored = get_task(&conn, tid).unwrap().unwrap().status;
            assert_eq!(stored, expected, "状态 {expected:?} 应能写库并读回");
        }
    }

    #[test]
    fn update_task_status_on_missing_task_is_ok() {
        let conn = mem_conn();
        update_task_status(&conn, 999, TaskStatus::Done).unwrap();
    }

    #[test]
    fn update_task_intent_persists_and_overwrites() {
        let conn = mem_conn();
        let (_, tid) = setup(&conn, "D:/code/demo");

        update_task_intent(&conn, tid, "为登录页补一个接口").unwrap();
        assert_eq!(
            get_task(&conn, tid).unwrap().unwrap().intent.as_deref(),
            Some("为登录页补一个接口")
        );

        update_task_intent(&conn, tid, "修正后的意图").unwrap();
        assert_eq!(
            get_task(&conn, tid).unwrap().unwrap().intent.as_deref(),
            Some("修正后的意图")
        );
    }

    #[test]
    fn update_task_plan_persists_and_overwrites() {
        let conn = mem_conn();
        let (_, tid) = setup(&conn, "D:/code/demo");

        update_task_plan(&conn, tid, "1. 定义结构\n2. 补测试").unwrap();
        assert_eq!(
            get_task(&conn, tid).unwrap().unwrap().plan.as_deref(),
            Some("1. 定义结构\n2. 补测试")
        );

        update_task_plan(&conn, tid, "v2 计划").unwrap();
        assert_eq!(
            get_task(&conn, tid).unwrap().unwrap().plan.as_deref(),
            Some("v2 计划")
        );
    }

    #[test]
    fn update_task_stats_overwrites_all_four_columns() {
        let conn = mem_conn();
        let (_, tid) = setup(&conn, "D:/code/demo");

        update_task_stats(&conn, tid, Some(1500), Some(120), Some(45), Some(0.002)).unwrap();
        let task = get_task(&conn, tid).unwrap().unwrap();
        assert_eq!(task.duration_ms, Some(1500));
        assert_eq!(task.token_input, Some(120));
        assert_eq!(task.token_output, Some(45));
        assert_eq!(task.cost, Some(0.002));

        // None 即清空对应列。
        update_task_stats(&conn, tid, None, Some(130), None, None).unwrap();
        let task = get_task(&conn, tid).unwrap().unwrap();
        assert_eq!(task.duration_ms, None);
        assert_eq!(task.token_input, Some(130));
        assert_eq!(task.token_output, None);
        assert_eq!(task.cost, None);
    }

    #[test]
    fn finish_task_sets_terminal_status_finished_at_and_duration() {
        let conn = mem_conn();
        let pid = project::insert_project(&conn, "D:/code/demo", "demo").unwrap();
        let done = insert_task(&conn, pid, "will succeed").unwrap();
        let failed = insert_task(&conn, pid, "will fail").unwrap();

        // 把 created_at 人为拨回 5 秒前，验证 duration_ms 的推算。
        conn.execute(
            "UPDATE tasks SET created_at = ?1 WHERE id IN (?2, ?3)",
            params![now_ms() - 5_000, done, failed],
        )
        .unwrap();

        finish_task(&conn, done, TaskStatus::Done).unwrap();
        finish_task(&conn, failed, TaskStatus::Failed).unwrap();

        let finished = get_task(&conn, done).unwrap().unwrap();
        assert_eq!(finished.status, TaskStatus::Done);
        assert!(finished.finished_at.is_some());
        let duration = finished.duration_ms.unwrap();
        assert!(
            (5_000..6_000).contains(&duration),
            "duration 应约 5 秒，实际 {duration}"
        );

        let aborted = get_task(&conn, failed).unwrap().unwrap();
        assert_eq!(aborted.status, TaskStatus::Failed);
        assert!(aborted.finished_at.is_some());
    }

    #[test]
    fn insert_step_appends_records_and_returns_ids() {
        let conn = mem_conn();
        let (_, tid) = setup(&conn, "D:/code/demo");

        let s1 = insert_step(&conn, tid, "intent", "解析用户意图").unwrap();
        let s2 = insert_step(&conn, tid, "plan", "生成执行计划").unwrap();
        assert!(s2 > s1);

        let first = &list_steps(&conn, tid).unwrap()[0];
        assert_eq!(first.task_id, tid);
        assert_eq!(first.step_type, "intent");
        assert_eq!(first.content, "解析用户意图");
        assert!(first.created_at > 1_600_000_000_000, "应为毫秒级时间戳");
    }

    #[test]
    fn insert_step_rejects_unknown_task_via_foreign_key() {
        let conn = mem_conn();
        let err = insert_step(&conn, 999, "plan", "orphan").unwrap_err();
        assert!(matches!(err, XaimError::Db(_)), "应由外键约束报数据库错误");
    }

    #[test]
    fn list_steps_returns_chronological_flow_of_that_task_only() {
        let conn = mem_conn();
        let pid = project::insert_project(&conn, "D:/code/demo", "demo").unwrap();
        let t1 = insert_task(&conn, pid, "task one").unwrap();
        let t2 = insert_task(&conn, pid, "task two").unwrap();

        let a = insert_step(&conn, t1, "intent", "first").unwrap();
        let b = insert_step(&conn, t1, "plan", "second").unwrap();
        let c = insert_step(&conn, t1, "retrieve", "third").unwrap();
        let other_step = insert_step(&conn, t2, "intent", "other task").unwrap();

        // 同一毫秒内按 id 正序兜底，整体即执行顺序。
        let ids: Vec<i64> = list_steps(&conn, t1)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec![a, b, c]);

        let other: Vec<i64> = list_steps(&conn, t2)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(other, vec![other_step]);
    }

    #[test]
    fn corrupted_status_string_yields_error_instead_of_panic() {
        let conn = mem_conn();
        let (_, tid) = setup(&conn, "D:/code/demo");
        conn.execute("UPDATE tasks SET status = 'bogus' WHERE id = ?1", [tid])
            .unwrap();

        let err = get_task(&conn, tid).unwrap_err();
        assert!(err.to_string().contains("未知的任务状态"));
    }
}
