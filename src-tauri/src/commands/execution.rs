use crate::agent::permissions::{Decision, PermissionRequest};
use crate::config::execution::{self, ExecutionMode, ExecutionSettings, GitNetwork};
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub fn get_execution_settings(
    project_id: i64,
    state: State<'_, AppState>,
) -> Result<ExecutionSettings, String> {
    execution::load(&state.store, project_id).map_err(String::from)
}

#[tauri::command]
pub fn set_execution_settings(
    project_id: i64,
    mode: ExecutionMode,
    git_network: GitNetwork,
    state: State<'_, AppState>,
) -> Result<ExecutionSettings, String> {
    execution::configure(&state.store, project_id, mode, git_network).map_err(String::from)
}

#[tauri::command]
pub fn revoke_execution_grant(
    project_id: i64,
    scope_id: String,
    state: State<'_, AppState>,
) -> Result<ExecutionSettings, String> {
    let settings = execution::revoke(&state.store, project_id, &scope_id).map_err(String::from)?;
    if let Some(agent) = state.running_orchestrator() {
        if let Some(session) = agent.permissions() {
            session.revoke(&scope_id);
        }
    }
    Ok(settings)
}

#[tauri::command]
pub fn get_pending_permission(state: State<'_, AppState>) -> Option<PermissionRequest> {
    state
        .running_orchestrator()
        .and_then(|agent| agent.permissions().and_then(|session| session.pending()))
}

#[tauri::command]
pub fn resolve_execution_permission(
    task_id: i64,
    request_id: String,
    decision: Decision,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let agent = state.running_orchestrator().ok_or("任务已停止")?;
    if agent.current_task_id() != Some(task_id) {
        return Err("授权请求不属于当前任务".into());
    }
    agent
        .permissions()
        .ok_or("当前任务不支持执行授权")?
        .resolve(task_id, &request_id, decision)
        .map_err(String::from)
}
