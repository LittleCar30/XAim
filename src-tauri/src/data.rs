//! App-owned data, restart-time migration, consistent backups and workspace retention.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::{Result, XaimError};
use crate::store::{self, Store};

pub const APP_ID: &str = "com.xaim.app";
const DATABASE: &str = "xaim.db";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    home: Option<PathBuf>,
    pending: Option<Migration>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Migration {
    source: PathBuf,
    target: PathBuf,
    #[serde(default)]
    restore: bool,
}

#[derive(Debug, Clone)]
pub struct DataPaths {
    pub home: PathBuf,
    pub bootstrap: PathBuf,
    pub overridden: bool,
    pub notice: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub home: String,
    pub database: String,
    pub workspaces: String,
    pub backups: String,
    pub bytes: u64,
    pub workspace_count: usize,
    pub retention_days: u32,
    pub pending_home: Option<String>,
    pub environment_override: bool,
    pub notice: Option<String>,
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| XaimError::Config("文件路径无效".into()))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec_pretty(value)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

impl DataPaths {
    /// Acquire before migration/recovery so a second process cannot recover a live task.
    pub fn acquire_process_lock() -> Result<fs::File> {
        let base = directories::BaseDirs::new()
            .ok_or_else(|| XaimError::Config("无法定位用户数据目录".into()))?;
        let bootstrap = base.data_dir().join(APP_ID);
        fs::create_dir_all(&bootstrap)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(bootstrap.join("app.lock"))?;
        lock.try_lock()
            .map_err(|_| XaimError::Config("XAim 已在运行，请使用已打开的窗口".into()))?;
        Ok(lock)
    }
    pub fn resolve() -> Result<Self> {
        let base = directories::BaseDirs::new()
            .ok_or_else(|| XaimError::Config("无法定位用户数据目录".into()))?;
        Self::resolve_in(
            base.data_dir(),
            std::env::var_os("XAIM_HOME").map(PathBuf::from),
        )
    }

    fn resolve_in(base: &Path, override_home: Option<PathBuf>) -> Result<Self> {
        let bootstrap = base.join(APP_ID);
        fs::create_dir_all(&bootstrap)?;
        let mut location = read_location(&bootstrap)?;
        let overridden = override_home.is_some();
        let mut home = override_home
            .or_else(|| location.home.clone())
            .unwrap_or_else(|| bootstrap.clone());
        if !home.is_absolute() {
            return Err(XaimError::Config("XAIM_HOME 必须是绝对路径".into()));
        }
        let mut notice = None;
        if !overridden {
            if let Some(pending) = &location.pending {
                let migration = if pending.restore {
                    validate_database(&pending.target.join(DATABASE))
                } else {
                    migrate(&pending.source, &pending.target)
                };
                match migration {
                    Ok(()) => {
                        home = pending.target.clone();
                        location.home = Some(home.clone());
                        location.pending = None;
                        atomic_json(&bootstrap.join("storage.json"), &location)?;
                        notice = Some("数据目录已切换，原目录已保留。".into());
                    }
                    Err(error) => {
                        home = pending.source.clone();
                        notice = Some(format!("数据迁移失败，继续使用原目录：{error}"));
                    }
                }
            }
            let legacy = base.join("com.xaim.desktop");
            if home == bootstrap && !home.join(DATABASE).exists() && legacy.join(DATABASE).is_file()
            {
                match migrate(&legacy, &home) {
                    Ok(()) => notice = Some("已迁移旧版配置和历史记录，旧目录已保留。".into()),
                    Err(error) => {
                        home = legacy;
                        notice = Some(format!("旧数据迁移失败，继续使用旧目录：{error}"));
                    }
                }
            }
        }
        for name in ["", "workspaces", "backups"] {
            fs::create_dir_all(home.join(name))?;
        }
        Ok(Self {
            home,
            bootstrap,
            overridden,
            notice,
        })
    }

    pub fn database(&self) -> PathBuf {
        self.home.join(DATABASE)
    }
    pub fn workspaces(&self) -> PathBuf {
        self.home.join("workspaces")
    }
    pub fn backups(&self) -> PathBuf {
        self.home.join("backups")
    }

    pub fn schedule_move(&self, target: &Path) -> Result<()> {
        if self.overridden {
            return Err(XaimError::Config(
                "当前由 XAIM_HOME 指定数据目录，请修改该环境变量后重启".into(),
            ));
        }
        if !target.is_absolute() {
            return Err(XaimError::Config("请选择绝对路径".into()));
        }
        fs::create_dir_all(target)?;
        let target = target.canonicalize()?;
        let source = self.home.canonicalize()?;
        if target == source || target.starts_with(&source) || source.starts_with(&target) {
            return Err(XaimError::Config("新目录必须独立于当前数据目录".into()));
        }
        if fs::read_dir(&target)?.next().is_some() {
            return Err(XaimError::Config(
                "请选择一个空文件夹，避免覆盖已有数据".into(),
            ));
        }
        let probe = tempfile::NamedTempFile::new_in(&target)?;
        probe.as_file().sync_all()?;
        let location = Location {
            home: Some(source.clone()),
            pending: Some(Migration {
                source,
                target,
                restore: false,
            }),
        };
        atomic_json(&self.bootstrap.join("storage.json"), &location)
    }

    /// Restore into a separate directory and switch on restart; current data stays intact.
    pub fn schedule_restore(&self, backup: &Path, target: &Path) -> Result<()> {
        validate_database(backup)?;
        self.schedule_move(target)?;
        let mut location = read_location(&self.bootstrap)?;
        let pending = location
            .pending
            .as_mut()
            .ok_or_else(|| XaimError::Config("无法安排恢复".into()))?;
        if let Err(error) = snapshot_database(backup, &pending.target.join(DATABASE)) {
            self.cancel_move()?;
            return Err(error);
        }
        pending.restore = true;
        atomic_json(&self.bootstrap.join("storage.json"), &location)
    }

    pub fn cancel_move(&self) -> Result<()> {
        let mut location = read_location(&self.bootstrap)?;
        location.pending = None;
        atomic_json(&self.bootstrap.join("storage.json"), &location)
    }

    pub fn info(&self, store: &Store) -> Result<StorageInfo> {
        let pending = read_location(&self.bootstrap)?
            .pending
            .map(|p| p.target.to_string_lossy().into_owned());
        Ok(StorageInfo {
            home: self.home.to_string_lossy().into_owned(),
            database: self.database().to_string_lossy().into_owned(),
            workspaces: self.workspaces().to_string_lossy().into_owned(),
            backups: self.backups().to_string_lossy().into_owned(),
            bytes: directory_size(&self.home)?,
            workspace_count: workspaces(&self.workspaces())?.len(),
            retention_days: retention_days(store)?,
            pending_home: pending,
            environment_override: self.overridden,
            notice: self.notice.clone(),
        })
    }

    pub fn backup(&self, store: &Store) -> Result<PathBuf> {
        crate::config::model::migrate_credentials(store)?;
        let name = format!(
            "xaim-{}-{}.db",
            chrono::Local::now().format("%Y%m%d-%H%M%S"),
            &uuid::Uuid::new_v4().to_string()[..8]
        );
        let path = self.backups().join(name);
        store.backup_to(&path)?;
        Ok(path)
    }
}

fn read_location(root: &Path) -> Result<Location> {
    let path = root.join("storage.json");
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Location::default()),
        Err(error) => Err(error.into()),
    }
}

fn migrate(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    if target.join(DATABASE).exists() {
        return Err(XaimError::Config("目标目录已有数据库，已停止迁移".into()));
    }
    // Put the database in place last. An interrupted copy can be retried safely.
    for name in ["workspaces", "backups"] {
        copy_directory(&source.join(name), &target.join(name))?;
    }
    snapshot_database(&source.join(DATABASE), &target.join(DATABASE))
}

fn validate_database(path: &Path) -> Result<()> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let health: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if health != "ok" {
        return Err(XaimError::Config("数据库完整性检查失败".into()));
    }
    for table in [
        "schema_version",
        "projects",
        "tasks",
        "task_steps",
        "patches",
        "cache",
        "files",
        "symbols",
    ] {
        let exists: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |row| row.get(0),
        )?;
        if exists != 1 {
            return Err(XaimError::Config("这不是完整的 XAim 数据备份".into()));
        }
    }
    let version: i64 = connection.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_version",
        [],
        |row| row.get(0),
    )?;
    if version > crate::store::schema::CURRENT_SCHEMA_VERSION {
        return Err(XaimError::Config("备份来自较新版本，请先升级 XAim".into()));
    }
    Ok(())
}

fn snapshot_database(source: &Path, destination: &Path) -> Result<()> {
    let connection =
        rusqlite::Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let parent = destination
        .parent()
        .ok_or_else(|| XaimError::Config("数据库路径无效".into()))?;
    let staging = tempfile::tempdir_in(parent)?;
    let staged_db = staging.path().join(DATABASE);
    connection.execute(
        "VACUUM main INTO ?1",
        [staged_db.to_string_lossy().as_ref()],
    )?;
    let check = rusqlite::Connection::open_with_flags(
        &staged_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let health: String = check.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if health != "ok" {
        return Err(XaimError::Config("迁移数据库完整性检查失败".into()));
    }
    drop(check);
    // Hard-link publication is atomic and refuses to replace an existing database.
    fs::hard_link(&staged_db, destination)?;
    Ok(())
}

fn copy_directory(source: &Path, target: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    if is_link(&fs::symlink_metadata(source)?) {
        return Err(XaimError::Security(
            "数据目录包含符号链接或联接，无法迁移".into(),
        ));
    }
    fs::create_dir_all(target)?;
    if is_link(&fs::symlink_metadata(target)?) {
        return Err(XaimError::Security("目标目录包含符号链接或联接".into()));
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if is_link(&metadata) {
            return Err(XaimError::Security(
                "数据目录包含符号链接或联接，无法迁移".into(),
            ));
        }
        let destination = target.join(entry.file_name());
        if metadata.is_dir() {
            copy_directory(&entry.path(), &destination)?;
        } else {
            if destination.exists() {
                if fs::read(&destination)? != fs::read(entry.path())? {
                    return Err(XaimError::Config("目标目录存在不同的文件，无法迁移".into()));
                }
            } else {
                fs::copy(entry.path(), destination)?;
            }
        }
    }
    Ok(())
}

fn directory_size(root: &Path) -> Result<u64> {
    let mut size = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if is_link(&metadata) {
            continue;
        }
        size += if metadata.is_dir() {
            directory_size(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(size)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceManifest {
    pub task_id: i64,
    pub project_path: PathBuf,
    pub created_at: i64,
}

fn workspaces(root: &Path) -> Result<Vec<(PathBuf, WorkspaceManifest)>> {
    let mut found = Vec::new();
    if !root.is_dir() {
        return Ok(found);
    }
    let canonical_root = root.canonicalize()?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if !metadata.is_dir()
            || is_link(&metadata)
            || !entry.file_name().to_string_lossy().starts_with("task-")
        {
            continue;
        }
        let path = entry.path().canonicalize()?;
        if path.parent() != Some(canonical_root.as_path()) {
            continue;
        }
        let manifest = path.join("workspace.json");
        if fs::symlink_metadata(&manifest).is_ok_and(|metadata| is_link(&metadata)) {
            continue;
        }
        if let Ok(bytes) = fs::read(manifest) {
            if let Ok(manifest) = serde_json::from_slice::<WorkspaceManifest>(&bytes) {
                found.push((path, manifest));
            }
        }
    }
    Ok(found)
}

pub fn task_workspace(paths: &DataPaths, task_id: i64) -> Result<PathBuf> {
    workspaces(&paths.workspaces())?
        .into_iter()
        .find(|(_, manifest)| manifest.task_id == task_id)
        .map(|(path, _)| path.join("files"))
        .ok_or_else(|| XaimError::Config("此任务没有保留工作区，可能已超过保留期限".into()))
}

pub fn retention_days(store: &Store) -> Result<u32> {
    Ok(
        store::cache::get_cache(&store.conn(), "storage:retentionDays")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(30),
    )
}

pub fn set_retention(store: &Store, days: u32) -> Result<()> {
    if days > 3650 {
        return Err(XaimError::Config(
            "保留期限应为 0 到 3650 天，0 表示永久保留".into(),
        ));
    }
    store::cache::set_cache(
        &store.conn(),
        "storage:retentionDays",
        &days.to_string(),
        None,
    )
}

pub fn cleanup(paths: &DataPaths, store: &Store) -> Result<usize> {
    let days = retention_days(store)?;
    if days == 0 {
        return Ok(0);
    }
    let cutoff = crate::utils::now_ms() - i64::from(days) * 86_400_000;
    let mut removed = 0;
    for (path, manifest) in workspaces(&paths.workspaces())? {
        let task = store::task::get_task(&store.conn(), manifest.task_id)?;
        // Pending review, unknown tasks and running tasks are never cleanup candidates.
        if task.is_some_and(|task| {
            task.status.is_terminal() && task.finished_at.is_some_and(|time| time < cutoff)
        }) {
            fs::remove_dir_all(path)?;
            removed += 1;
        }
    }
    store::cache::clear_expired(&store.conn())?;
    Ok(removed)
}

pub fn recover_interrupted_tasks(store: &Store) -> Result<usize> {
    let conn = store.conn();
    let tx = conn.unchecked_transaction()?;
    let ids: Vec<i64> = {
        let mut query = tx.prepare("SELECT id FROM tasks WHERE status NOT IN ('done','failed','blocked','cancelled','rolledBack','waitingConfirm','applied')")?;
        let rows = query
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    for id in &ids {
        store::task::finish_task(&tx, *id, crate::types::TaskStatus::Failed)?;
        let event = serde_json::json!({"type":"message","taskId":id,"content":"上次运行意外中断，已保留任务工作区和最近检查点，可导出记录或查看工作区。"});
        store::task::insert_step(&tx, *id, "event", &event.to_string())?;
    }
    tx.commit()?;
    Ok(ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::workspace::TaskWorkspace;
    use crate::types::TaskStatus;

    fn project(store: &Store, root: &Path) -> i64 {
        store::project::insert_project(&store.conn(), &root.to_string_lossy(), "test").unwrap()
    }

    #[test]
    fn legacy_migration_includes_live_wal_and_never_replaces_existing_data() {
        let base = tempfile::tempdir().unwrap();
        let old = base.path().join("com.xaim.desktop");
        let legacy = Store::new(&old.join(DATABASE)).unwrap();
        legacy
            .conn()
            .execute_batch("PRAGMA wal_autocheckpoint=0")
            .unwrap();
        store::cache::set_cache(&legacy.conn(), "existing", "from-wal", None).unwrap();
        let paths = DataPaths::resolve_in(base.path(), None).unwrap();
        assert!(paths.home.ends_with(APP_ID));
        let migrated = Store::new(&paths.database()).unwrap();
        assert_eq!(
            store::cache::get_cache(&migrated.conn(), "existing")
                .unwrap()
                .as_deref(),
            Some("from-wal")
        );
        store::cache::set_cache(&migrated.conn(), "existing", "newer", None).unwrap();
        let reopened = DataPaths::resolve_in(base.path(), None).unwrap();
        assert_eq!(reopened.home, paths.home);
        assert_eq!(
            store::cache::get_cache(&migrated.conn(), "existing")
                .unwrap()
                .as_deref(),
            Some("newer")
        );
        assert!(old.join(DATABASE).exists());
    }

    #[test]
    fn move_reads_latest_data_at_restart_and_keeps_workspaces_and_source() {
        let base = tempfile::tempdir().unwrap();
        let paths = DataPaths::resolve_in(base.path(), None).unwrap();
        let store = Store::new(&paths.database()).unwrap();
        let project_root = tempfile::tempdir().unwrap();
        fs::write(project_root.path().join("a.txt"), "before").unwrap();
        let project_id = project(&store, project_root.path());
        let task_id = store::task::insert_task(&store.conn(), project_id, "fix").unwrap();
        let workspace =
            TaskWorkspace::managed_snapshot(project_root.path(), &paths.workspaces(), task_id)
                .unwrap();
        fs::write(workspace.root().join("a.txt"), "after").unwrap();
        drop(workspace);
        let target = base.path().join("custom");
        paths.schedule_move(&target).unwrap();
        store::cache::set_cache(&store.conn(), "late-write", "keep", None).unwrap();
        drop(store);
        let moved = DataPaths::resolve_in(base.path(), None).unwrap();
        let store = Store::new(&moved.database()).unwrap();
        assert_eq!(
            moved.home.canonicalize().unwrap(),
            target.canonicalize().unwrap()
        );
        assert_eq!(
            store::cache::get_cache(&store.conn(), "late-write")
                .unwrap()
                .as_deref(),
            Some("keep")
        );
        assert_eq!(
            fs::read_to_string(task_workspace(&moved, task_id).unwrap().join("a.txt")).unwrap(),
            "after"
        );
        assert!(paths.database().is_file());
        assert!(moved.info(&store).unwrap().pending_home.is_none());
    }

    #[test]
    fn failed_migration_falls_back_without_overwriting_either_database() {
        let base = tempfile::tempdir().unwrap();
        let paths = DataPaths::resolve_in(base.path(), None).unwrap();
        let source = Store::new(&paths.database()).unwrap();
        let target = base.path().join("target");
        paths.schedule_move(&target).unwrap();
        let other = Store::new(&target.join(DATABASE)).unwrap();
        store::cache::set_cache(&other.conn(), "keep", "other", None).unwrap();
        let resolved = DataPaths::resolve_in(base.path(), None).unwrap();
        assert_eq!(
            resolved.home.canonicalize().unwrap(),
            paths.home.canonicalize().unwrap()
        );
        assert!(resolved.notice.unwrap().contains("失败"));
        assert_eq!(
            store::cache::get_cache(&other.conn(), "keep")
                .unwrap()
                .as_deref(),
            Some("other")
        );
        assert!(source
            .backup_to(&base.path().join("still-readable.db"))
            .is_ok());
    }

    #[test]
    fn backup_restores_in_an_independent_directory_without_touching_current_data() {
        let base = tempfile::tempdir().unwrap();
        let paths = DataPaths::resolve_in(base.path(), None).unwrap();
        let source = Store::new(&paths.database()).unwrap();
        store::cache::set_cache(&source.conn(), "pref:xaim.theme", "dark", None).unwrap();
        let backup = paths.backup(&source).unwrap();
        store::cache::set_cache(&source.conn(), "pref:xaim.theme", "light", None).unwrap();
        let target = base.path().join("restore");
        paths.schedule_restore(&backup, &target).unwrap();
        assert_eq!(
            store::cache::get_cache(&source.conn(), "pref:xaim.theme")
                .unwrap()
                .as_deref(),
            Some("light")
        );
        let restored = DataPaths::resolve_in(base.path(), None).unwrap();
        let database = Store::new(&restored.database()).unwrap();
        assert_eq!(
            store::cache::get_cache(&database.conn(), "pref:xaim.theme")
                .unwrap()
                .as_deref(),
            Some("dark")
        );
        assert!(paths.database().is_file());
        assert!(
            source.backup_to(&backup).is_err(),
            "backups must never overwrite an existing file"
        );
    }

    #[test]
    fn validates_migration_targets_override_and_retention() {
        let base = tempfile::tempdir().unwrap();
        let paths = DataPaths::resolve_in(base.path(), None).unwrap();
        let store = Store::new(&paths.database()).unwrap();
        assert!(paths.schedule_move(&paths.home).is_err());
        assert!(paths.schedule_move(&paths.home.join("nested")).is_err());
        assert!(paths.schedule_move(base.path()).is_err());
        assert!(paths.schedule_move(Path::new("relative")).is_err());
        let override_home = base.path().join("override");
        let override_paths =
            DataPaths::resolve_in(base.path(), Some(override_home.clone())).unwrap();
        assert_eq!(override_paths.home, override_home);
        assert!(override_paths
            .schedule_move(&base.path().join("other"))
            .is_err());
        assert!(DataPaths::resolve_in(base.path(), Some("relative".into())).is_err());
        assert!(set_retention(&store, 3651).is_err());
        set_retention(&store, 0).unwrap();
        assert_eq!(retention_days(&store).unwrap(), 0);
    }

    #[test]
    fn cleanup_only_removes_expired_terminal_workspaces_and_retains_history() {
        let base = tempfile::tempdir().unwrap();
        let paths = DataPaths::resolve_in(base.path(), None).unwrap();
        let store = Store::new(&paths.database()).unwrap();
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), "source").unwrap();
        let project_id = project(&store, root.path());
        let statuses = [
            TaskStatus::Done,
            TaskStatus::WaitingConfirm,
            TaskStatus::Blocked,
            TaskStatus::Generating,
            TaskStatus::Cancelled,
        ];
        let mut ids = Vec::new();
        for status in statuses {
            let id = store::task::insert_task(&store.conn(), project_id, "test").unwrap();
            store::task::update_task_status(&store.conn(), id, status).unwrap();
            store
                .conn()
                .execute(
                    "UPDATE tasks SET finished_at=?1 WHERE id=?2",
                    rusqlite::params![crate::utils::now_ms() - 40 * 86_400_000_i64, id],
                )
                .unwrap();
            drop(TaskWorkspace::managed_snapshot(root.path(), &paths.workspaces(), id).unwrap());
            ids.push(id);
        }
        set_retention(&store, 0).unwrap();
        assert_eq!(cleanup(&paths, &store).unwrap(), 0);
        set_retention(&store, 30).unwrap();
        assert_eq!(cleanup(&paths, &store).unwrap(), 3);
        assert!(task_workspace(&paths, ids[0]).is_err());
        assert!(task_workspace(&paths, ids[1]).is_ok());
        assert!(task_workspace(&paths, ids[2]).is_err());
        assert!(task_workspace(&paths, ids[3]).is_ok());
        assert!(store::task::get_task(&store.conn(), ids[0])
            .unwrap()
            .is_some());
        assert_eq!(
            fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "source"
        );
    }

    #[test]
    fn interrupted_runs_become_failed_but_pending_review_survives() {
        let store = Store::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let project_id = project(&store, root.path());
        let interrupted = store::task::insert_task(&store.conn(), project_id, "running").unwrap();
        let waiting = store::task::insert_task(&store.conn(), project_id, "waiting").unwrap();
        store::task::update_task_status(&store.conn(), interrupted, TaskStatus::Generating)
            .unwrap();
        store::task::update_task_status(&store.conn(), waiting, TaskStatus::WaitingConfirm)
            .unwrap();
        store::cache::set_cache(
            &store.conn(),
            &format!("task:{interrupted}:partial_changes"),
            "durable",
            None,
        )
        .unwrap();
        assert_eq!(recover_interrupted_tasks(&store).unwrap(), 1);
        assert_eq!(recover_interrupted_tasks(&store).unwrap(), 0);
        assert_eq!(
            store::task::get_task(&store.conn(), interrupted)
                .unwrap()
                .unwrap()
                .status,
            TaskStatus::Failed
        );
        assert_eq!(
            store::task::get_task(&store.conn(), waiting)
                .unwrap()
                .unwrap()
                .status,
            TaskStatus::WaitingConfirm
        );
        assert_eq!(
            store::cache::get_cache(
                &store.conn(),
                &format!("task:{interrupted}:partial_changes")
            )
            .unwrap()
            .as_deref(),
            Some("durable")
        );
    }
}
