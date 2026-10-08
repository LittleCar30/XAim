//! 全局应用状态：持久化存储、当前打开的项目与任务编排器。
//!
//! 启动时经 Tauri `app.manage` 注入，命令层通过 `State<'_, AppState>` 拿到。
//! [`Store`] 内部自带互斥锁，可克隆共享；当前项目至多一个——再次打开会
//! 替换旧项目，旧项目的文件监听器随 [`OpenProject`] 被 drop 而停止。
//! 任务同一时间至多运行一个（[`AppState::try_begin_run`] 保证）；运行
//! 产生的编排器按任务 id 保留，供确认/拒绝/回滚复用其事件出口。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use notify::RecommendedWatcher;

use crate::agent::orchestrator::Orchestrator;
use crate::errors::XaimError;
use crate::store::Store;
use crate::types::Project;

/// 当前打开的项目与其文件变更监听器；监听器 drop 即停止增量索引事件。
pub struct OpenProject {
    pub project: Project,
    _watcher: RecommendedWatcher,
}

/// Tauri 全局状态。
pub struct AppState {
    /// SQLite 存储：项目/文件/符号/任务等全部持久化数据。
    pub store: Store,
    current: Mutex<Option<OpenProject>>,
    /// 正在运行的任务编排器；`None` 即空闲。
    running: Mutex<Option<Orchestrator>>,
    /// 本会话内产生过任务的编排器，按任务 id 保留。
    orchestrators: Mutex<HashMap<i64, Orchestrator>>,
}

impl AppState {
    /// 用默认数据目录下的 SQLite 库构建应用状态（应用启动入口）。
    ///
    /// # Panics
    /// 数据目录无法定位或数据库无法打开时 panic——启动期不可恢复，
    /// 与 `main` 里其他启动失败的 `expect` 同一口径。
    // 不提供 Default：new() 做可失败的 IO，panic 不应藏在 trait 方法背后。
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let dir = default_data_dir().expect("无法定位应用数据目录");
        let store = Store::new(&dir.join("xaim.db")).expect("无法打开应用数据库");
        Self::with_store(store)
    }

    /// 用给定的存储构建状态（测试与自定义路径用）。
    pub fn with_store(store: Store) -> Self {
        Self {
            store,
            current: Mutex::new(None),
            running: Mutex::new(None),
            orchestrators: Mutex::new(HashMap::new()),
        }
    }

    /// 占用运行槽开始一个任务；已有任务在运行时返回 [`XaimError::Config`]。
    pub fn try_begin_run(&self, orchestrator: Orchestrator) -> Result<(), XaimError> {
        let mut slot = self.lock_running();
        if slot.is_some() {
            return Err(XaimError::Config(
                "已有任务正在运行，请等待其完成或先取消".into(),
            ));
        }
        *slot = Some(orchestrator);
        Ok(())
    }

    /// 任务运行结束（无论成败）释放运行槽。
    pub fn end_run(&self) {
        *self.lock_running() = None;
    }

    /// 当前正在运行任务的编排器；空闲时返回 `None`。
    pub fn running_orchestrator(&self) -> Option<Orchestrator> {
        self.lock_running().clone()
    }

    /// 记录已完成 `run_task` 的编排器（按任务 id），供确认/拒绝/回滚。
    pub fn remember_orchestrator(&self, task_id: i64, orchestrator: Orchestrator) {
        self.lock_orchestrators().insert(task_id, orchestrator);
    }

    /// 取任务对应的编排器；本会话内未运行过该任务时返回 `None`。
    pub fn orchestrator_for(&self, task_id: i64) -> Option<Orchestrator> {
        self.lock_orchestrators().get(&task_id).cloned()
    }

    /// 当前打开的项目；未打开返回 `None`。
    pub fn current_project(&self) -> Option<Project> {
        self.lock_current()
            .as_ref()
            .map(|open| open.project.clone())
    }

    /// 记录新打开的项目及其监听器；若此前已有项目打开，旧监听器随替换被 drop。
    pub fn set_current_project(&self, project: Project, watcher: RecommendedWatcher) {
        *self.lock_current() = Some(OpenProject {
            project,
            _watcher: watcher,
        });
    }

    /// 关闭当前项目并返回它；未打开时返回 `None`（关闭幂等）。
    pub fn take_current_project(&self) -> Option<Project> {
        self.lock_current().take().map(|open| open.project)
    }

    /// 拿当前项目的锁；锁中毒时恢复内部值，一处 panic 不应让状态永久不可用
    /// （与 `Store::conn` 同一口径）。
    fn lock_current(&self) -> MutexGuard<'_, Option<OpenProject>> {
        self.current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_running(&self) -> MutexGuard<'_, Option<Orchestrator>> {
        self.running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_orchestrators(&self) -> MutexGuard<'_, HashMap<i64, Orchestrator>> {
        self.orchestrators
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 应用默认数据目录（存放 SQLite 库等）：与 Tauri `app_data_dir` 同一口径，
/// 即系统数据目录 + 应用标识 `com.xaim.desktop`（Windows 为
/// `%APPDATA%\com.xaim.desktop`）。
fn default_data_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.data_dir().join("com.xaim.desktop"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    use tempfile::tempdir;
    use tokio::sync::mpsc;

    use crate::agent::orchestrator::{Orchestrator, OrchestratorConfig};
    use crate::errors::Result;
    use crate::events::XaimEvent;
    use crate::index::watcher;
    use crate::model::provider::MockProvider;
    use crate::repair::r#loop::EventSink;

    /// 编译期断言：Tauri `manage` 要求 `Send + Sync + 'static`。
    #[test]
    fn app_state_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AppState>();
    }

    #[test]
    fn default_data_dir_resolves_to_app_folder() {
        let dir = default_data_dir().expect("常规环境下应能解析数据目录");
        assert!(
            dir.ends_with("com.xaim.desktop"),
            "应与 Tauri app_data_dir 同口径: {}",
            dir.display()
        );
    }

    /// 给 `root` 挂一个真实的监听器（`set_current_project` 需要）。
    fn watcher_for(root: &Path) -> RecommendedWatcher {
        let (tx, _rx) = mpsc::channel(4);
        watcher::start_watcher(root.to_path_buf(), tx).unwrap()
    }

    fn project(id: i64) -> Project {
        Project {
            id,
            path: format!("D:/code/p{id}"),
            name: format!("p{id}"),
            created_at: 0,
            last_opened_at: 0,
        }
    }

    /// 供编排器构造用的空事件出口。
    struct NopSink;

    impl EventSink for NopSink {
        fn emit(&self, _event: XaimEvent) -> Result<()> {
            Ok(())
        }
    }

    /// 最小可用的编排器实例（不发起任何模型调用）。
    fn orchestrator(store: &Store) -> Orchestrator {
        Orchestrator::with_sink(
            Arc::new(store.clone()),
            OrchestratorConfig::default(),
            Arc::new(NopSink),
            Arc::new(MockProvider::new("unused")),
        )
    }

    #[test]
    fn run_slot_is_exclusive_until_released() {
        let state = AppState::with_store(Store::open_in_memory().unwrap());

        state.try_begin_run(orchestrator(&state.store)).unwrap();
        let err = state.try_begin_run(orchestrator(&state.store)).unwrap_err();
        assert!(
            matches!(err, XaimError::Config(_)),
            "并发运行应被拒绝: {err:?}"
        );

        state.end_run();
        state
            .try_begin_run(orchestrator(&state.store))
            .expect("释放后应可再次运行");
    }

    #[test]
    fn running_orchestrator_tracks_the_slot() {
        let state = AppState::with_store(Store::open_in_memory().unwrap());
        assert!(state.running_orchestrator().is_none());

        state.try_begin_run(orchestrator(&state.store)).unwrap();
        assert!(state.running_orchestrator().is_some());

        state.end_run();
        assert!(state.running_orchestrator().is_none());
    }

    #[test]
    fn remembered_orchestrators_are_looked_up_by_task_id() {
        let state = AppState::with_store(Store::open_in_memory().unwrap());

        state.remember_orchestrator(5, orchestrator(&state.store));
        assert!(state.orchestrator_for(5).is_some());
        assert!(state.orchestrator_for(6).is_none(), "未记录的任务应查不到");
    }

    #[test]
    fn current_project_is_none_initially() {
        let state = AppState::with_store(Store::open_in_memory().unwrap());
        assert!(state.current_project().is_none());
        assert!(state.take_current_project().is_none());
    }

    #[test]
    fn set_then_take_returns_the_project() {
        let dir = tempdir().unwrap();
        let state = AppState::with_store(Store::open_in_memory().unwrap());

        state.set_current_project(project(1), watcher_for(dir.path()));
        assert_eq!(state.current_project().map(|p| p.id), Some(1));
        assert_eq!(state.take_current_project().map(|p| p.id), Some(1));
        assert!(state.current_project().is_none(), "take 之后应为空");
    }

    #[test]
    fn reopening_replaces_previous_project() {
        let dir = tempdir().unwrap();
        let state = AppState::with_store(Store::open_in_memory().unwrap());

        state.set_current_project(project(1), watcher_for(dir.path()));
        state.set_current_project(project(2), watcher_for(dir.path()));
        assert_eq!(state.current_project().map(|p| p.id), Some(2));
    }

    #[test]
    fn closing_twice_is_idempotent() {
        let dir = tempdir().unwrap();
        let state = AppState::with_store(Store::open_in_memory().unwrap());

        state.set_current_project(project(7), watcher_for(dir.path()));
        assert_eq!(state.take_current_project().map(|p| p.id), Some(7));
        assert!(state.take_current_project().is_none());
    }
}
