use std::collections::BTreeMap;
use std::path::Path;

use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_shell::ShellExt;

use crate::data::{self, DataPaths, StorageInfo};
use crate::state::AppState;
use crate::store;

fn paths(state: &AppState) -> Result<DataPaths, String> {
    state
        .data_paths
        .clone()
        .ok_or_else(|| "数据目录不可用".into())
}

#[tauri::command]
pub async fn get_storage_info(state: State<'_, AppState>) -> Result<StorageInfo, String> {
    let paths = paths(&state)?;
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || paths.info(&store).map_err(String::from))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn open_data_folder(
    kind: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let paths = paths(&state)?;
    let folder = match kind.as_str() {
        "home" => paths.home,
        "workspaces" => paths.workspaces(),
        "backups" => paths.backups(),
        _ => return Err("未知的数据目录".into()),
    };
    #[allow(deprecated)]
    app.shell()
        .open(folder.to_string_lossy().as_ref(), None)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn pick_storage_folder(app: AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择新的空数据文件夹")
        .pick_folder(move |folder| {
            let _ = sender.send(folder);
        });
    receiver
        .await
        .map_err(|error| error.to_string())?
        .map(|folder| {
            folder
                .into_path()
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|error| error.to_string())
        })
        .transpose()
}

#[tauri::command]
pub async fn move_data_directory(path: String, state: State<'_, AppState>) -> Result<(), String> {
    paths(&state)?
        .schedule_move(Path::new(&path))
        .map_err(String::from)
}

#[tauri::command]
pub async fn cancel_data_move(state: State<'_, AppState>) -> Result<(), String> {
    paths(&state)?.cancel_move().map_err(String::from)
}

#[tauri::command]
pub async fn create_data_backup(state: State<'_, AppState>) -> Result<String, String> {
    let paths = paths(&state)?;
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || {
        paths
            .backup(&store)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(String::from)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn restore_data_backup(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let paths = paths(&state)?;
    if paths.overridden {
        return Err("当前由 XAIM_HOME 指定数据目录，请先移除该环境变量再恢复备份".into());
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择 XAim 数据备份")
        .add_filter("XAim 数据库", &["db"])
        .pick_file(move |file| {
            let _ = sender.send(file);
        });
    let Some(backup) = receiver.await.map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    let backup = backup.into_path().map_err(|error| error.to_string())?;
    let Some(target) = pick_storage_folder(app).await? else {
        return Ok(None);
    };
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || {
        paths.backup(&store).map_err(String::from)?;
        paths
            .schedule_restore(&backup, Path::new(&target))
            .map_err(String::from)?;
        Ok(Some(target))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn set_workspace_retention(days: u32, state: State<'_, AppState>) -> Result<(), String> {
    data::set_retention(&state.store, days).map_err(String::from)
}

#[tauri::command]
pub async fn cleanup_workspaces(state: State<'_, AppState>) -> Result<usize, String> {
    let paths = paths(&state)?;
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || data::cleanup(&paths, &store).map_err(String::from))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn open_task_workspace(
    task_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let path = data::task_workspace(&paths(&state)?, task_id).map_err(String::from)?;
    #[allow(deprecated)]
    app.shell()
        .open(path.to_string_lossy().as_ref(), None)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn open_task_project(
    task_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let project_path = {
        let conn = state.store.conn();
        let task = store::task::get_task(&conn, task_id)
            .map_err(String::from)?
            .ok_or_else(|| "任务不存在".to_string())?;
        store::project::get_project(&conn, task.project_id)
            .map_err(String::from)?
            .ok_or_else(|| "任务所属项目不存在".to_string())?
            .path
    };
    #[allow(deprecated)]
    app.shell()
        .open(&project_path, None)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn export_task_record(
    task_id: i64,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("导出对话与任务记录")
        .set_file_name(format!("xaim-task-{task_id}.json"))
        .add_filter("任务记录", &["json"])
        .save_file(move |file| {
            let _ = sender.send(file);
        });
    let Some(file) = receiver.await.map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    let destination = file.into_path().map_err(|error| error.to_string())?;
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
        let conn = store.conn();
        let task = store::task::get_task(&conn, task_id).map_err(String::from)?.ok_or("任务不存在")?;
        let steps = store::task::list_steps(&conn, task_id).map_err(String::from)?;
        let patches = store::patch::list_patches_by_task(&conn, task_id).map_err(String::from)?;
        let report = store::cache::get_cache(&conn, &format!("task:{task_id}:report")).map_err(String::from)?;
        let checkpoint = store::cache::get_cache(&conn, &format!("task:{task_id}:snapshot")).map_err(String::from)?;
        drop(conn);
        let value = serde_json::json!({"version":1,"task":task,"steps":steps,"patches":patches,
            "report": report.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
            "checkpoint": checkpoint.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())});
        data::atomic_json(&destination, &value).map_err(String::from)?;
        Ok(Some(destination.to_string_lossy().into_owned()))
    }).await.map_err(|error| error.to_string())?
}

const PREFERENCE_KEYS: &[&str] = &["xaim.theme", "xaim.settings-prefs", "xaim.prompt-history"];

#[tauri::command]
pub async fn get_preferences(
    state: State<'_, AppState>,
) -> Result<BTreeMap<String, String>, String> {
    preferences(&state.store)
}

pub(super) fn preferences(store: &store::Store) -> Result<BTreeMap<String, String>, String> {
    let conn = store.conn();
    let mut values = BTreeMap::new();
    for key in PREFERENCE_KEYS {
        if let Some(value) =
            store::cache::get_cache(&conn, &format!("pref:{key}")).map_err(String::from)?
        {
            values.insert((*key).into(), value);
        }
    }
    Ok(values)
}

#[tauri::command]
pub async fn set_preference(
    key: String,
    value: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    save_preference(&state.store, &key, &value)
}

pub(super) fn save_preference(store: &store::Store, key: &str, value: &str) -> Result<(), String> {
    if !PREFERENCE_KEYS.contains(&key) || value.len() > 256 * 1024 {
        return Err("偏好设置无效或过大".into());
    }
    store::cache::set_cache(&store.conn(), &format!("pref:{key}"), value, None)
        .map_err(String::from)
}
