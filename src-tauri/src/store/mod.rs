//! 持久化层：SQLite 存储入口与各领域仓库（项目/文件/符号/任务/补丁/缓存）。
//!
//! [`Store`] 是全局唯一入口：内部持有一把互斥锁保护的 SQLite 连接，
//! 可在多线程间共享（`Clone` 出的实例与原实例共用同一连接）。
//! 打开时自动开启 WAL 与外键约束，并执行幂等的 schema 迁移（见 [`schema`]）。

pub mod cache;
pub mod file;
pub mod patch;
pub mod project;
pub mod schema;
pub mod task;

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;

use crate::errors::Result;

/// SQLite 存储入口。
///
/// - `new` 打开（不存在则创建）数据库文件，自动建库并迁移到最新 schema；
/// - `conn` 独占借用连接执行任意 SQL；表结构见 [`schema`]。
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// 打开（不存在则创建）`path` 处的 SQLite 数据库。
    ///
    /// 父目录不存在时会自动创建；首次运行自动建表，之后每次打开都
    /// 幂等地把 schema 迁移到 [`schema::CURRENT_SCHEMA_VERSION`]。
    pub fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Self::init(Connection::open(path)?)
    }

    /// 打开内存数据库（测试与临时计算用），连接关闭即消失。
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // WAL 允许读写并发，适合主线程 + 后台索引任务同时访问；
        // 外键约束必须逐连接开启，ON DELETE CASCADE 才会生效。
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // 并发访问同一文件库时等待锁而不是立刻报 SQLITE_BUSY。
        conn.busy_timeout(Duration::from_secs(5))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.migrate()?;
        Ok(store)
    }

    /// 把数据库 schema 迁移到 [`schema::CURRENT_SCHEMA_VERSION`]（幂等）。
    pub fn migrate(&self) -> Result<()> {
        schema::migrate(&self.conn())
    }

    /// 独占借用底层连接，用于执行任意 SQL。
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        // 锁中毒时恢复内部值：一处 panic 不应让整个存储层永久不可用。
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// SQLite itself includes committed WAL data; never copy a live .db file.
    pub fn backup_to(&self, destination: &Path) -> Result<()> {
        let parent = destination
            .parent()
            .ok_or_else(|| crate::errors::XaimError::Config("备份路径无效".into()))?;
        std::fs::create_dir_all(parent)?;
        let temporary = tempfile::NamedTempFile::new_in(parent)?;
        let temporary_path = temporary.into_temp_path();
        std::fs::remove_file(&temporary_path)?;
        self.conn().execute(
            "VACUUM main INTO ?1",
            [temporary_path.to_string_lossy().as_ref()],
        )?;
        let check = Connection::open_with_flags(
            &temporary_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let result: String = check.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        drop(check);
        if result != "ok" {
            return Err(crate::errors::XaimError::Config(
                "备份完整性检查失败".into(),
            ));
        }
        temporary_path
            .persist_noclobber(destination)
            .map_err(|error| error.error)?;
        Ok(())
    }
}

/// 供 store 各子模块测试共用的工具。
#[cfg(test)]
pub(crate) mod testing {
    use rusqlite::Connection;

    /// 返回已完成迁移的内存数据库。
    pub(crate) fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        super::schema::migrate(&conn).unwrap();
        conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn crud_survives_reopen_and_cascade_is_project_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/store.db");
        let (project_id, other_id, task_id, patch_id, file_id) = {
            let store = Store::new(&path).unwrap();
            let conn = store.conn();
            let project_id = project::insert_project(&conn, "project-a", "a").unwrap();
            let other_id = project::insert_project(&conn, "project-b", "b").unwrap();
            let task_id = task::insert_task(&conn, project_id, "update login").unwrap();
            task::insert_step(&conn, task_id, "plan", "update auth").unwrap();
            task::update_task_plan(&conn, task_id, "plan v2").unwrap();
            let patch_id = patch::insert_patch(&conn, task_id, "auth.rs", "diff", None).unwrap();
            patch::update_score(&conn, patch_id, Some(0.9)).unwrap();
            patch::mark_applied(&conn, patch_id).unwrap();
            let file_id = file::upsert_file(
                &conn,
                project_id,
                "auth.rs",
                Some("rust"),
                Some(10),
                Some(1),
                Some("old"),
            )
            .unwrap();
            file::update_summary(&conn, file_id, Some("auth summary")).unwrap();
            assert_eq!(
                file::upsert_file(
                    &conn,
                    project_id,
                    "auth.rs",
                    Some("rust"),
                    Some(20),
                    Some(2),
                    Some("new"),
                )
                .unwrap(),
                file_id
            );
            conn.execute(
                "INSERT INTO symbols (file_id, name, kind, start_line, end_line) VALUES (?1, 'login', 'function', 1, 2)",
                [file_id],
            ).unwrap();
            file::upsert_file(&conn, other_id, "auth.rs", None, None, None, None).unwrap();
            cache::set_cache(&conn, "shared", "v1", None).unwrap();
            cache::set_cache(&conn, "shared", "v2", None).unwrap();
            (project_id, other_id, task_id, patch_id, file_id)
        };

        let store = Store::new(&path).unwrap();
        let conn = store.conn();
        let record = file::get_file(&conn, project_id, "auth.rs")
            .unwrap()
            .unwrap();
        assert_eq!(record.id, file_id);
        assert_eq!(record.size, Some(20));
        assert_eq!(record.hash.as_deref(), Some("new"));
        assert_eq!(record.summary.as_deref(), Some("auth summary"));
        assert_eq!(
            task::get_task(&conn, task_id)
                .unwrap()
                .unwrap()
                .plan
                .as_deref(),
            Some("plan v2")
        );
        assert_eq!(
            task::list_steps(&conn, task_id).unwrap()[0].content,
            "update auth"
        );
        let patch = patch::get_patch(&conn, patch_id).unwrap().unwrap();
        assert!(patch.applied);
        assert_eq!(patch.score, Some(0.9));
        assert_eq!(
            cache::get_cache(&conn, "shared").unwrap().as_deref(),
            Some("v2")
        );

        project::delete_project(&conn, project_id).unwrap();
        assert!(project::get_project(&conn, project_id).unwrap().is_none());
        assert!(file::list_files(&conn, project_id).unwrap().is_empty());
        assert!(task::get_task(&conn, task_id).unwrap().is_none());
        assert!(task::list_steps(&conn, task_id).unwrap().is_empty());
        assert!(patch::get_patch(&conn, patch_id).unwrap().is_none());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(project::get_project(&conn, other_id).unwrap().is_some());
        assert_eq!(file::list_files(&conn, other_id).unwrap().len(), 1);
        cache::delete_cache(&conn, "shared").unwrap();
        assert!(cache::get_cache(&conn, "shared").unwrap().is_none());
    }

    /// 在临时目录里生成一个数据库文件路径（目录随 TempDir 一起清理）。
    fn tmp_db_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("xaim.db");
        (dir, path)
    }

    #[test]
    fn new_creates_database_and_tables_on_first_run() {
        let (_dir, path) = tmp_db_path();
        assert!(!path.exists());
        let store = Store::new(&path).unwrap();
        assert!(path.exists(), "首次运行应自动创建数据库文件");

        let conn = store.conn();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
                 ('projects', 'files', 'symbols', 'tasks', 'task_steps', 'patches', 'cache', 'schema_version')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(n, 8, "全部 8 张表都应存在");
    }

    #[test]
    fn reopening_existing_database_succeeds_and_keeps_data() {
        let (_dir, path) = tmp_db_path();
        {
            let store = Store::new(&path).unwrap();
            store
                .conn()
                .execute(
                    "INSERT INTO projects (path, name, created_at, last_opened_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        "D:/code/demo",
                        "demo",
                        1_727_568_000_000i64,
                        1_727_568_000_000i64
                    ],
                )
                .unwrap();
        }

        // 第二次运行不报错，且首次写入的数据仍在、版本未被重复登记。
        let store = Store::new(&path).unwrap();
        let (projects, versions): (i64, i64) = store
            .conn()
            .query_row(
                "SELECT (SELECT COUNT(*) FROM projects), (SELECT COUNT(*) FROM schema_version)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(projects, 1);
        assert_eq!(
            versions,
            crate::store::schema::CURRENT_SCHEMA_VERSION,
            "每个 schema 版本应恰好登记一次"
        );
    }

    #[test]
    fn file_database_uses_wal_mode() {
        let (_dir, path) = tmp_db_path();
        let store = Store::new(&path).unwrap();
        let mode: String = store
            .conn()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
    }

    #[test]
    fn foreign_keys_are_enabled() {
        let store = Store::open_in_memory().unwrap();
        let enabled: i64 = store
            .conn()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(enabled, 1);
    }

    #[test]
    fn clone_shares_the_same_connection() {
        let (_dir, path) = tmp_db_path();
        let store = Store::new(&path).unwrap();
        let clone = store.clone();
        clone
            .conn()
            .execute(
                "INSERT INTO projects (path, name, created_at, last_opened_at)
                 VALUES ('p', 'n', 0, 0)",
                [],
            )
            .unwrap();

        let count: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "克隆实例应共享同一数据库连接");
    }
}
