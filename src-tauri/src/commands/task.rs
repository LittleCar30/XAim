//! 任务相关命令：运行/取消/查询任务，与补丁的确认、拒绝、回滚。
//!
//! 业务逻辑委托给 [`Orchestrator`]（完整流程、确认、拒绝、回滚），
//! 命令层只做编排器定位、参数转发与错误转 `String`。
//!
//! 会话模型：`run_task` 同一时间只允许一个任务在运行，返回即代表流程
//! 走到等待确认（或失败）；产生的编排器按任务 id 保留在 [`AppState`]，
//! 后续的确认/拒绝/回滚借它复用事件出口——应用重启后未确认的任务
//! 需重新运行才能继续操作。

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::agent::orchestrator::{Orchestrator, OrchestratorConfig};
use crate::errors::XaimError;
use crate::state::AppState;
use crate::store::{self, Store};
use crate::types::{Patch, Task, TaskStep};

/// 任务历史的单次返回上限（按创建时间倒序取最新）。
const TASK_HISTORY_LIMIT: usize = 200;

/// 运行编码任务，在隔离副本中执行工具循环与最终验证，到达等待确认后返回。
///
/// 模型与路由取用户保存的配置（`set_config` / `set_model_config`）。
/// 进度经事件通道实时推送（step/patch/error/done）；同一时间只允许
/// 一个任务在运行，期间可经 [`cancel_task`] 在步骤边界取消。
#[tauri::command]
pub async fn run_task(
    project_id: i64,
    prompt: String,
    allow_verification: Option<bool>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    let mut config = orchestrator_config(&state)?;
    config.tool_loop.allow_verification = allow_verification.unwrap_or(false);
    let orchestrator =
        Orchestrator::new(app, Arc::new(state.store.clone()), config).map_err(String::from)?;
    run(&state, orchestrator, project_id, &prompt)
        .await
        .map_err(String::from)
}

/// 从存储中的用户配置构建编排配置（模型连接 + 路由；其余步骤参数用默认值）。
pub(super) fn orchestrator_config(state: &AppState) -> Result<OrchestratorConfig, XaimError> {
    let app = crate::config::model::load(&state.store);
    orchestrator_config_from(app)
}

pub(crate) fn orchestrator_config_from(
    app: crate::types::Config,
) -> Result<OrchestratorConfig, XaimError> {
    crate::config::model::validate(&app)?;
    let selected = app
        .model_profiles
        .iter()
        .find(|p| Some(&p.id) == app.active_model_id.as_ref());
    let mut connection = crate::config::model::ModelConfig::from(app.model.clone());
    let mut router = app.router;
    router.fast_model = app.model.name.clone();
    router.strong_model = app.model.name.clone();
    router.overrides.clear();
    if let Some(profile) = selected {
        connection.reasoning_model = profile.supports_reasoning;
        connection.reasoning_effort = profile.reasoning_effort.clone();
    }
    let limits = crate::model::capabilities::limits(
        &connection.provider,
        &app.model.name,
        connection.reasoning_model,
    );
    let output = connection
        .max_tokens
        .unwrap_or(limits.default_output_tokens);
    let ceiling = if limits.documented {
        output.min(limits.max_output_tokens)
    } else {
        output
    };
    let loop_config = crate::agent::tool_loop::LoopConfig {
        output_tokens: ceiling,
        output_ceiling: connection
            .max_tokens
            .unwrap_or(limits.max_output_tokens.min(65_536)),
        // Reserve headroom for wire metadata; tool schemas and results are counted in the loop.
        context_tokens: limits.context_tokens.saturating_sub(4096),
        ..Default::default()
    };
    Ok(OrchestratorConfig {
        model: connection,
        router,
        tool_loop: loop_config,
        ..OrchestratorConfig::default()
    })
}

/// 请求取消正在运行的任务：流程在下一个步骤边界停止并转入 Cancelled；
/// 已结束的任务报错。
#[tauri::command]
pub async fn cancel_task(task_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    cancel(&state, task_id).await.map_err(String::from)
}

/// 按 id 查任务；不存在时报错。
#[tauri::command]
pub async fn get_task(task_id: i64, state: State<'_, AppState>) -> Result<Task, String> {
    get(&state, task_id).await.map_err(String::from)
}

/// 某项目的任务历史，按创建时间倒序（最新在前），最多 [`TASK_HISTORY_LIMIT`] 条。
#[tauri::command]
pub async fn list_tasks(project_id: i64, state: State<'_, AppState>) -> Result<Vec<Task>, String> {
    list(&state, project_id).await.map_err(String::from)
}

#[tauri::command]
pub async fn list_task_steps(
    task_id: i64,
    state: State<'_, AppState>,
) -> Result<Vec<TaskStep>, String> {
    steps(&state, task_id).await.map_err(String::from)
}

/// 应用已审阅的任务变更，保留 Git 历史与其他工作区修改。
#[tauri::command]
pub async fn apply_patch(patch_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    apply(&state, patch_id).await.map_err(String::from)
}

/// 拒绝候选补丁：不改动工作区，任务转入 Cancelled。
#[tauri::command]
pub async fn reject_patch(patch_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    reject(&state, patch_id).await.map_err(String::from)
}

/// 撤销任务自身的文件变更；文件已被其他操作修改时拒绝覆盖。
#[tauri::command]
pub async fn rollback_task(task_id: i64, state: State<'_, AppState>) -> Result<(), String> {
    rollback(&state, task_id).await.map_err(String::from)
}

/// 某任务的补丁列表，按创建时间倒序（最新候选在前）。
#[tauri::command]
pub async fn list_patches(task_id: i64, state: State<'_, AppState>) -> Result<Vec<Patch>, String> {
    patches(&state, task_id).await.map_err(String::from)
}

/// `run_task` 的实现：占运行槽 → 驱动完整流程 → 释放运行槽并按任务 id
/// 记住编排器（供确认/拒绝/回滚复用其事件出口）。
pub(super) async fn run(
    state: &AppState,
    orchestrator: Orchestrator,
    project_id: i64,
    prompt: &str,
) -> Result<i64, XaimError> {
    state.try_begin_run(orchestrator.clone())?;
    let outcome = orchestrator.run_task(project_id, prompt.to_string()).await;
    state.end_run();
    let task_id = outcome?;
    state.remember_orchestrator(task_id, orchestrator);
    Ok(task_id)
}

/// `cancel_task` 的实现：终态任务与不在运行中的任务都明确报错。
async fn cancel(state: &AppState, task_id: i64) -> Result<(), XaimError> {
    let task = {
        let conn = state.store.conn();
        store::task::get_task(&conn, task_id)?
    }
    .ok_or_else(|| XaimError::Config(format!("任务 #{task_id} 不存在")))?;
    if task.status.is_terminal() {
        return Err(XaimError::Config(format!(
            "任务 #{task_id} 已结束（{:?}），无需取消",
            task.status
        )));
    }
    let running = state
        .running_orchestrator()
        .ok_or_else(|| XaimError::Config(format!("任务 #{task_id} 不在当前会话中运行")))?;
    if running.current_task_id() != Some(task_id) {
        return Err(XaimError::Config(format!(
            "任务 #{task_id} 不在运行中，当前运行的是任务 #{}",
            running.current_task_id().unwrap_or_default()
        )));
    }
    running.cancel().await;
    Ok(())
}

/// `get_task` 的实现。
pub(super) async fn get(state: &AppState, task_id: i64) -> Result<Task, XaimError> {
    let conn = state.store.conn();
    store::task::get_task(&conn, task_id)?
        .ok_or_else(|| XaimError::Config(format!("任务 #{task_id} 不存在")))
}

/// `list_tasks` 的实现。
pub(super) async fn list(state: &AppState, project_id: i64) -> Result<Vec<Task>, XaimError> {
    let conn = state.store.conn();
    store::task::list_tasks(&conn, project_id, TASK_HISTORY_LIMIT)
}

pub(super) async fn steps(state: &AppState, task_id: i64) -> Result<Vec<TaskStep>, XaimError> {
    let conn = state.store.conn();
    store::task::get_task(&conn, task_id)?
        .ok_or_else(|| XaimError::Config(format!("任务 #{task_id} 不存在")))?;
    store::task::list_steps(&conn, task_id)
}

/// `apply_patch` 的实现：补丁 → 所属任务 → 该任务的编排器 → 确认。
pub(super) async fn apply(state: &AppState, patch_id: i64) -> Result<(), XaimError> {
    let task_id = patch_task_id(&state.store, patch_id).await?;
    let orchestrator = state
        .orchestrator_for(task_id)
        .ok_or_else(|| not_operable(task_id))?;
    orchestrator.confirm_task(task_id).await
}

/// `reject_patch` 的实现。
async fn reject(state: &AppState, patch_id: i64) -> Result<(), XaimError> {
    let task_id = patch_task_id(&state.store, patch_id).await?;
    let orchestrator = state
        .orchestrator_for(task_id)
        .ok_or_else(|| not_operable(task_id))?;
    orchestrator.reject_task(task_id).await
}

/// `rollback_task` 的实现：任务存在性先行（给出明确错误），再定位会话内编排器。
async fn rollback(state: &AppState, task_id: i64) -> Result<(), XaimError> {
    {
        let conn = state.store.conn();
        store::task::get_task(&conn, task_id)?
            .ok_or_else(|| XaimError::Config(format!("任务 #{task_id} 不存在")))?;
    }
    let orchestrator = state
        .orchestrator_for(task_id)
        .ok_or_else(|| not_operable(task_id))?;
    orchestrator.rollback_task(task_id).await
}

/// `list_patches` 的实现。
pub(super) async fn patches(state: &AppState, task_id: i64) -> Result<Vec<Patch>, XaimError> {
    let conn = state.store.conn();
    store::patch::list_patches_by_task(&conn, task_id)
}

/// 查补丁所属任务 id；补丁不存在时返回 [`XaimError::Config`]。
async fn patch_task_id(store: &Store, patch_id: i64) -> Result<i64, XaimError> {
    let conn = store.conn();
    store::patch::get_patch(&conn, patch_id)?
        .map(|patch| patch.task_id)
        .ok_or_else(|| XaimError::Config(format!("补丁 #{patch_id} 不存在")))
}

/// 补丁操作找不到会话内编排器时的统一错误。
fn not_operable(task_id: i64) -> XaimError {
    XaimError::Config(format!(
        "任务 #{task_id} 尚未完成或不在当前会话中，无法操作其补丁"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::fs;
    use std::path::Path;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::time::Duration;

    use futures::Stream;
    use tempfile::tempdir;

    use crate::events::XaimEvent;
    use crate::model::provider::{
        CompletionRequest, CompletionResponse, MockProvider, ModelProvider,
    };
    use crate::repair::r#loop::EventSink;
    use crate::types::TaskStatus;

    #[test]
    fn configured_model_limits_and_output_ceiling_reach_the_executor() {
        let mut config = crate::types::Config::default();
        config.model.provider = "deepseek".into();
        config.model.name = "deepseek-flash".into();
        config.model.max_tokens = Some(16_384);
        let runtime = orchestrator_config_from(config).unwrap();
        assert_eq!(runtime.tool_loop.context_tokens, 1_000_000 - 4096);
        assert_eq!(runtime.tool_loop.output_tokens, 16_384);
        assert_eq!(runtime.tool_loop.output_ceiling, 16_384);
        assert!(!runtime.tool_loop.allow_verification);
    }

    const BROKEN_LIB: &str = "pub fn add(a: i32, b: i32) -> i32 { a + }\n";
    const FIXED_LIB: &str = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";
    const FIX_DIFF: &str = "--- a/src/lib.rs\n\
                            +++ b/src/lib.rs\n\
                            @@ -1 +1 @@\n\
                            -pub fn add(a: i32, b: i32) -> i32 { a + }\n\
                            +pub fn add(a: i32, b: i32) -> i32 { a + b }\n";

    const INTENT_JSON: &str = r#"{"action":"fix_bug","target":"add","description":"修复 add 函数的语法错误","constraints":[],"verify":["test"]}"#;
    const PLAN_JSON: &str = r#"{"steps":[{"action":"modify","file":"src/lib.rs","reason":"修复坏表达式"},{"action":"test","file":"cargo test","reason":"验证修复"}],"files_to_modify":["src/lib.rs"],"risks":[]}"#;
    const CONFIDENCE_JSON: &str = r#"{"confidence": 0.5}"#;

    /// 内存库 + 空任务槽的测试状态；返回共享的存储句柄。
    fn setup() -> (AppState, Arc<Store>) {
        let store = Store::open_in_memory().unwrap();
        let state = AppState::with_store(store.clone());
        (state, Arc::new(store))
    }

    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// 登记指向 `root` 的项目，返回 project_id。
    fn register_project(store: &Store, root: &Path) -> i64 {
        let conn = store.conn();
        store::project::insert_project(&conn, root.to_str().unwrap(), "demo").unwrap()
    }

    /// 造一个处于等待确认阶段的任务（含一份未应用补丁），返回 (task_id, patch_id)。
    fn waiting_task(store: &Store, root: &Path) -> (i64, i64) {
        let project_id = register_project(store, root);
        let conn = store.conn();
        let task_id = store::task::insert_task(&conn, project_id, "修复 add").unwrap();
        store::task::update_task_status(&conn, task_id, TaskStatus::WaitingConfirm).unwrap();
        let patch_id =
            store::patch::insert_patch(&conn, task_id, "src/lib.rs", FIX_DIFF, Some(0.9)).unwrap();
        let changes = crate::agent::workspace::ChangeSet {
            files: vec![crate::agent::workspace::FileChange {
                path: "src/lib.rs".into(),
                before: Some(BROKEN_LIB.into()),
                after: Some(FIXED_LIB.into()),
            }],
        };
        store::cache::set_cache(
            &conn,
            &format!("task:{task_id}:changes"),
            &serde_json::to_string(&changes).unwrap(),
            None,
        )
        .unwrap();
        (task_id, patch_id)
    }

    /// 最小编排器：事件进收集器，provider 固定（确认/拒绝/回滚不调用模型）。
    fn orchestrator_with(
        store: &Arc<Store>,
        sink: CollectSink,
        provider: Arc<dyn ModelProvider>,
    ) -> Orchestrator {
        Orchestrator::with_sink(
            store.clone(),
            OrchestratorConfig::default(),
            Arc::new(sink),
            provider,
        )
    }

    /// 查任务状态。
    fn task_status(store: &Store, task_id: i64) -> TaskStatus {
        let conn = store.conn();
        store::task::get_task(&conn, task_id)
            .unwrap()
            .expect("任务应存在")
            .status
    }

    /// 取某项目最新的任务 id（单任务运行时即运行中的任务）。
    fn latest_task_id(store: &Store, project_id: i64) -> i64 {
        let conn = store.conn();
        store::task::list_tasks(&conn, project_id, 10).unwrap()[0].id
    }

    // -- run_task -----------------------------------------------------------

    #[tokio::test]
    async fn run_reaches_waiting_confirm_and_remembers_orchestrator() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", BROKEN_LIB);
        let (state, store) = setup();
        let project_id = register_project(&store, dir.path());
        let sink = CollectSink::default();
        // 调用顺序：意图 x1 + 计划 x1 + 候选 x1 + 置信度 x1。
        let provider = Arc::new(QueueProvider::new(&[
            INTENT_JSON,
            PLAN_JSON,
            FIX_DIFF,
            CONFIDENCE_JSON,
        ]));
        let config = OrchestratorConfig {
            candidate_count: 1,
            ..Default::default()
        };
        let orchestrator =
            Orchestrator::with_sink(store.clone(), config, Arc::new(sink.clone()), provider);

        let task_id = run(&state, orchestrator, project_id, "修复 add 函数的语法错误")
            .await
            .unwrap();
        let task = {
            let conn = store.conn();
            store::task::get_task(&conn, task_id).unwrap().unwrap()
        };
        assert_eq!(task.status, TaskStatus::WaitingConfirm);
        assert!(task.intent.is_some() && task.plan.is_some());
        assert!(
            state.orchestrator_for(task_id).is_some(),
            "运行产生的编排器应按任务 id 保留"
        );
        assert!(state.running_orchestrator().is_none(), "结束后应释放运行槽");
        assert!(sink.has_event("patch", ""), "应推送补丁事件");
        assert!(!sink.has_event("done", ""), "到达等待确认不算终态");
    }

    #[tokio::test]
    async fn run_rejects_concurrent_second_task() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", BROKEN_LIB);
        let (state, store) = setup();
        let project_id = register_project(&store, dir.path());
        let provider = Arc::new(
            QueueProvider::new(&[INTENT_JSON, PLAN_JSON, FIX_DIFF, CONFIDENCE_JSON])
                .with_delay(Duration::from_millis(300)),
        );
        let orchestrator = orchestrator_with(&store, CollectSink::default(), provider);

        // spawn 要求 'static：用 Arc 共享状态给后台任务。
        let state = Arc::new(state);
        let first = tokio::spawn({
            let state = state.clone();
            let orchestrator = orchestrator.clone();
            async move { run(&state, orchestrator, project_id, "第一个任务").await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let second = run(&state, orchestrator.clone(), project_id, "第二个任务")
            .await
            .unwrap_err();
        assert!(
            second.to_string().contains("已有任务正在运行"),
            "并发任务应被拒绝: {second}"
        );

        first.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn run_failure_releases_the_run_slot() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", BROKEN_LIB);
        let (state, store) = setup();
        let project_id = register_project(&store, dir.path());
        // 意图解析拿到垃圾输出：步骤失败，任务转 Failed。
        let orchestrator = orchestrator_with(
            &store,
            CollectSink::default(),
            Arc::new(QueueProvider::new(&["这不是 JSON"])),
        );

        let err = run(&state, orchestrator, project_id, "修复 add")
            .await
            .unwrap_err();
        assert!(matches!(err, XaimError::Model(_)), "实际 {err:?}");
        assert!(state.running_orchestrator().is_none(), "失败也应释放运行槽");
        let conn = store.conn();
        let task = &store::task::list_tasks(&conn, project_id, 10).unwrap()[0];
        assert_eq!(task.status, TaskStatus::Failed);
    }

    #[tokio::test]
    async fn run_missing_project_creates_no_task() {
        let (state, store) = setup();
        let provider = Arc::new(MockProvider::new("unused"));
        let orchestrator = orchestrator_with(&store, CollectSink::default(), provider);

        let err = run(&state, orchestrator, 999, "任意").await.unwrap_err();
        assert!(matches!(err, XaimError::Config(_)), "实际 {err:?}");
        assert!(state.running_orchestrator().is_none());
        let conn = store.conn();
        assert!(store::task::list_tasks(&conn, 999, 10).unwrap().is_empty());
    }

    // -- cancel_task --------------------------------------------------------

    #[tokio::test]
    async fn cancel_stops_the_running_task() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", BROKEN_LIB);
        let (state, store) = setup();
        let project_id = register_project(&store, dir.path());
        // 每次模型调用 300ms：取消发生在调用进行中，下一个步骤边界生效。
        let provider = Arc::new(
            QueueProvider::new(&[INTENT_JSON, PLAN_JSON, FIX_DIFF, CONFIDENCE_JSON])
                .with_delay(Duration::from_millis(300)),
        );
        let orchestrator = orchestrator_with(&store, CollectSink::default(), provider);

        let state = Arc::new(state);
        let handle = tokio::spawn({
            let state = state.clone();
            let orchestrator = orchestrator.clone();
            async move { run(&state, orchestrator, project_id, "修复 add").await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let task_id = latest_task_id(&store, project_id);

        cancel(&state, task_id).await.unwrap();

        let result = handle.await.unwrap();
        assert!(
            matches!(result, Err(XaimError::Cancelled)),
            "实际 {result:?}"
        );
        assert_eq!(task_status(&store, task_id), TaskStatus::Cancelled);
        assert!(state.running_orchestrator().is_none());
        assert_eq!(
            fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
            BROKEN_LIB,
            "取消不应改动工作区"
        );
    }

    #[tokio::test]
    async fn cancel_covers_finished_unknown_and_other_task() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", BROKEN_LIB);
        let (state, store) = setup();
        let project_id = register_project(&store, dir.path());
        let (old_task, _patch) = waiting_task(&store, dir.path());

        // 不存在的任务。
        let err = cancel(&state, 999).await.unwrap_err();
        assert!(err.to_string().contains("不存在"), "{err}");

        // 已结束的任务（人为置为终态）。
        {
            let conn = store.conn();
            store::task::finish_task(&conn, old_task, TaskStatus::Done).unwrap();
        }
        let err = cancel(&state, old_task).await.unwrap_err();
        assert!(err.to_string().contains("已结束"), "{err}");

        // 非终态但没有任务在运行。
        let (pending, _) = waiting_task(&store, dir.path());
        let err = cancel(&state, pending).await.unwrap_err();
        assert!(err.to_string().contains("不在当前会话中运行"), "{err}");

        // 运行中的是别的任务：不能张冠李戴。
        let provider = Arc::new(
            QueueProvider::new(&[INTENT_JSON, PLAN_JSON, FIX_DIFF, CONFIDENCE_JSON])
                .with_delay(Duration::from_millis(300)),
        );
        let orchestrator = orchestrator_with(&store, CollectSink::default(), provider);
        let state = Arc::new(state);
        let handle = tokio::spawn({
            let state = state.clone();
            let orchestrator = orchestrator.clone();
            async move { run(&state, orchestrator, project_id, "第三个任务").await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let running_id = latest_task_id(&store, project_id);
        let (waiting_again, _) = waiting_task(&store, dir.path());
        let err = cancel(&state, waiting_again).await.unwrap_err();
        assert!(err.to_string().contains("不在运行中"), "{err}");

        // 取消真正在运行的任务并收尾。
        cancel(&state, running_id).await.unwrap();
        let result = handle.await.unwrap();
        assert!(
            matches!(result, Err(XaimError::Cancelled)),
            "实际 {result:?}"
        );
    }

    // -- get / list ---------------------------------------------------------

    #[tokio::test]
    async fn get_task_returns_record_or_errors() {
        let dir = tempdir().unwrap();
        let (state, store) = setup();
        let project_id = register_project(&store, dir.path());
        let task_id = {
            let conn = store.conn();
            store::task::insert_task(&conn, project_id, "add login").unwrap()
        };

        let task = get(&state, task_id).await.unwrap();
        assert_eq!(task.id, task_id);
        assert_eq!(task.prompt, "add login");

        let err = get(&state, 999).await.unwrap_err();
        assert!(err.to_string().contains("任务 #999 不存在"), "{err}");
    }

    #[tokio::test]
    async fn list_tasks_returns_newest_first_for_the_project() {
        let dir = tempdir().unwrap();
        let (state, store) = setup();
        let project_id = register_project(&store, dir.path());
        let other = register_project(&store, Path::new("D:/code/other"));
        let (t1, t2) = {
            let conn = store.conn();
            (
                store::task::insert_task(&conn, project_id, "first").unwrap(),
                store::task::insert_task(&conn, project_id, "second").unwrap(),
            )
        };
        {
            let conn = store.conn();
            store::task::insert_task(&conn, other, "unrelated").unwrap();
        }

        let ids: Vec<i64> = list(&state, project_id)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![t2, t1], "最新在前且只含本项目");
    }

    #[tokio::test]
    async fn list_task_steps_reads_persisted_history_and_rejects_unknown_tasks() {
        let dir = tempdir().unwrap();
        let (state, store) = setup();
        let project_id = register_project(&store, dir.path());
        let task_id = {
            let conn = store.conn();
            let task_id = store::task::insert_task(&conn, project_id, "history").unwrap();
            store::task::insert_step(&conn, task_id, "plan", "done").unwrap();
            store::task::insert_step(&conn, task_id, "verify", "done").unwrap();
            task_id
        };
        let records = steps(&state, task_id).await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].step_type, "plan");
        assert_eq!(records[1].step_type, "verify");
        assert!(steps(&state, 999)
            .await
            .unwrap_err()
            .to_string()
            .contains("不存在"));
    }

    // -- apply / reject / rollback ------------------------------------------

    #[tokio::test]
    async fn apply_applies_reviewed_changes_without_creating_git_commits() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", BROKEN_LIB);
        let (state, store) = setup();
        let (task_id, patch_id) = waiting_task(&store, dir.path());
        let sink = CollectSink::default();
        state.remember_orchestrator(
            task_id,
            orchestrator_with(&store, sink.clone(), Arc::new(MockProvider::new("unused"))),
        );

        apply(&state, patch_id).await.unwrap();

        assert_eq!(task_status(&store, task_id), TaskStatus::Done);
        let conn = store.conn();
        let patch = store::patch::get_patch(&conn, patch_id).unwrap().unwrap();
        assert!(patch.applied, "补丁应标记为已应用");
        drop(conn);
        assert_eq!(
            fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
            FIXED_LIB,
            "补丁内容应已落盘"
        );
        assert!(
            !dir.path().join(".git").exists(),
            "确认不应自动初始化或提交 Git"
        );
        assert!(sink.has_event("done", "done"), "应推送完成事件");
    }

    #[tokio::test]
    async fn apply_rejects_unknown_patch_and_missing_orchestrator() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", BROKEN_LIB);
        let (state, store) = setup();
        let (_task_id, patch_id) = waiting_task(&store, dir.path());

        let err = apply(&state, 999).await.unwrap_err();
        assert!(err.to_string().contains("补丁 #999 不存在"), "{err}");

        let err = apply(&state, patch_id).await.unwrap_err();
        assert!(
            err.to_string().contains("尚未完成或不在当前会话中"),
            "没有会话内编排器应报错: {err}"
        );
        assert_eq!(task_status(&store, _task_id), TaskStatus::WaitingConfirm);
    }

    #[tokio::test]
    async fn apply_twice_fails_on_finished_task() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", BROKEN_LIB);
        let (state, store) = setup();
        let (task_id, patch_id) = waiting_task(&store, dir.path());
        state.remember_orchestrator(
            task_id,
            orchestrator_with(
                &store,
                CollectSink::default(),
                Arc::new(MockProvider::new("x")),
            ),
        );

        apply(&state, patch_id).await.unwrap();
        let err = apply(&state, patch_id).await.unwrap_err();
        assert!(
            err.to_string().contains("不在等待确认阶段"),
            "重复应用应报状态错误: {err}"
        );
    }

    #[tokio::test]
    async fn reject_marks_cancelled_without_touching_workspace() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", BROKEN_LIB);
        let (state, store) = setup();
        let (task_id, patch_id) = waiting_task(&store, dir.path());
        let sink = CollectSink::default();
        state.remember_orchestrator(
            task_id,
            orchestrator_with(&store, sink.clone(), Arc::new(MockProvider::new("unused"))),
        );

        reject(&state, patch_id).await.unwrap();

        assert_eq!(task_status(&store, task_id), TaskStatus::Cancelled);
        let conn = store.conn();
        let patch = store::patch::get_patch(&conn, patch_id).unwrap().unwrap();
        assert!(!patch.applied, "拒绝的补丁应保持未应用");
        drop(conn);
        assert_eq!(
            fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
            BROKEN_LIB,
            "拒绝不应改动工作区"
        );
        assert!(sink.has_event("done", "cancelled"), "应推送取消终态事件");
    }

    #[tokio::test]
    async fn reject_rejects_finished_task_and_unknown_patch() {
        let dir = tempdir().unwrap();
        let (state, store) = setup();
        let (task_id, patch_id) = waiting_task(&store, dir.path());
        state.remember_orchestrator(
            task_id,
            orchestrator_with(
                &store,
                CollectSink::default(),
                Arc::new(MockProvider::new("x")),
            ),
        );

        let err = reject(&state, 999).await.unwrap_err();
        assert!(err.to_string().contains("补丁 #999 不存在"), "{err}");

        {
            let conn = store.conn();
            store::task::finish_task(&conn, task_id, TaskStatus::Done).unwrap();
        }
        let err = reject(&state, patch_id).await.unwrap_err();
        assert!(err.to_string().contains("不在等待确认阶段"), "{err}");
    }

    #[tokio::test]
    async fn rollback_restores_workspace_and_marks_rolled_back() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "src/lib.rs", BROKEN_LIB);
        // 预置一个初始提交：任务提交才有父提交可回。
        crate::git::ops::commit(dir.path(), "init").unwrap();

        let (state, store) = setup();
        let (task_id, patch_id) = waiting_task(&store, dir.path());
        let sink = CollectSink::default();
        state.remember_orchestrator(
            task_id,
            orchestrator_with(&store, sink.clone(), Arc::new(MockProvider::new("unused"))),
        );
        apply(&state, patch_id).await.unwrap();
        assert_eq!(task_status(&store, task_id), TaskStatus::Done);

        rollback(&state, task_id).await.unwrap();

        assert_eq!(
            task_status(&store, task_id),
            TaskStatus::RolledBack,
            "已完成任务的回滚属用户事后撤销，终态重写为 rolledBack"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
            BROKEN_LIB,
            "工作区应回到任务提交之前"
        );
        assert!(
            crate::git::ops::status(dir.path()).unwrap().is_empty(),
            "回滚后工作区应干净"
        );
        assert!(sink.has_event("done", "rolledBack"), "应推送回滚终态事件");
    }

    #[tokio::test]
    async fn rollback_rejects_task_without_applied_patch() {
        let dir = tempdir().unwrap();
        let (state, store) = setup();
        let (task_id, _patch_id) = waiting_task(&store, dir.path());
        state.remember_orchestrator(
            task_id,
            orchestrator_with(
                &store,
                CollectSink::default(),
                Arc::new(MockProvider::new("x")),
            ),
        );

        let err = rollback(&state, 999).await.unwrap_err();
        assert!(err.to_string().contains("任务 #999 不存在"), "{err}");

        let err = rollback(&state, task_id).await.unwrap_err();
        assert!(
            err.to_string().contains("没有已应用的补丁可回滚"),
            "等待确认阶段无补丁可回滚: {err}"
        );
    }

    // -- list_patches -------------------------------------------------------

    #[tokio::test]
    async fn list_patches_returns_that_tasks_patches_newest_first() {
        let dir = tempdir().unwrap();
        let (state, store) = setup();
        let project_id = register_project(&store, dir.path());
        let (t1, t2) = {
            let conn = store.conn();
            (
                store::task::insert_task(&conn, project_id, "one").unwrap(),
                store::task::insert_task(&conn, project_id, "two").unwrap(),
            )
        };
        let (p1, p2) = {
            let conn = store.conn();
            (
                store::patch::insert_patch(&conn, t1, "a.rs", "--- a", None).unwrap(),
                store::patch::insert_patch(&conn, t1, "b.rs", "--- b", None).unwrap(),
            )
        };
        {
            let conn = store.conn();
            store::patch::insert_patch(&conn, t2, "c.rs", "--- c", None).unwrap();
        }

        let ids: Vec<i64> = patches(&state, t1)
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec![p2, p1], "最新在前且只含本任务");
        assert!(patches(&state, 999).await.unwrap().is_empty());
    }

    // -- 测试基础设施 -------------------------------------------------------

    /// 按脚本依次返回响应的 provider：响应耗尽后重复最后一条；
    /// 每次调用先休眠 `delay`，供并发与取消测试控制节奏。
    struct QueueProvider {
        responses: Mutex<VecDeque<String>>,
        delay: Duration,
    }

    impl QueueProvider {
        fn new(responses: &[&str]) -> Self {
            Self {
                responses: Mutex::new(responses.iter().map(|s| s.to_string()).collect()),
                delay: Duration::ZERO,
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }
    }

    #[async_trait::async_trait]
    impl ModelProvider for QueueProvider {
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, XaimError> {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let mut queue = self.responses.lock().unwrap();
            let response = if queue.len() > 1 {
                queue.pop_front().unwrap()
            } else {
                queue[0].clone()
            };
            Ok(CompletionResponse {
                content: response,
                usage: Default::default(),
            })
        }

        async fn stream(
            &self,
            _req: CompletionRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<String, XaimError>> + Send>>, XaimError>
        {
            unimplemented!("命令层测试只使用 complete")
        }

        fn name(&self) -> &str {
            "queue"
        }

        fn max_context(&self) -> usize {
            8192
        }
    }

    /// 测试事件收集器：事件序列化为 JSON 存入内存，供事件流断言。
    #[derive(Default, Clone)]
    struct CollectSink(Arc<Mutex<Vec<serde_json::Value>>>);

    impl CollectSink {
        fn has_event(&self, event_type: &str, status: &str) -> bool {
            self.0
                .lock()
                .unwrap()
                .iter()
                .any(|v| v["type"] == event_type && (status.is_empty() || v["status"] == status))
        }
    }

    impl EventSink for CollectSink {
        fn emit(&self, event: XaimEvent) -> Result<(), XaimError> {
            self.0.lock().unwrap().push(serde_json::to_value(&event)?);
            Ok(())
        }
    }
}
