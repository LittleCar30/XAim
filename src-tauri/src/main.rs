// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! XAim 桌面端入口：初始化日志，构建 Tauri 应用并注册全部 IPC 命令。
//!
//! 命令实现见库 crate（`xaim_lib`）的 `commands` 模块；全局状态
//! [`AppState`] 持有 SQLite 存储与当前项目/任务编排器。

use tauri::Manager;
use xaim_lib::commands;
use xaim_lib::state::AppState;

fn main() {
    init_logging();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            app.manage(AppState::new());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::project::open_project,
            commands::project::pick_project_folder,
            commands::project::close_project,
            commands::project::list_projects,
            commands::project::get_file_tree,
            commands::project::read_file,
            commands::task::run_task,
            commands::task::cancel_task,
            commands::task::get_task,
            commands::task::list_tasks,
            commands::task::list_task_steps,
            commands::task::list_conversation_tasks,
            commands::task::read_task_file,
            commands::task::apply_patch,
            commands::task::reject_patch,
            commands::task::rollback_task,
            commands::task::list_patches,
            commands::model::get_model_config,
            commands::model::set_model_config,
            commands::model::test_model_connection,
            commands::model::list_models,
            commands::model::list_providers,
            commands::config::get_config,
            commands::config::set_config,
            commands::data::get_storage_info,
            commands::data::open_data_folder,
            commands::data::pick_storage_folder,
            commands::data::move_data_directory,
            commands::data::cancel_data_move,
            commands::data::create_data_backup,
            commands::data::restore_data_backup,
            commands::data::set_workspace_retention,
            commands::data::cleanup_workspaces,
            commands::data::open_task_workspace,
            commands::data::export_task_record,
            commands::data::get_preferences,
            commands::data::set_preference,
            xaim_lib::config::runtime::get_runtime_settings,
            xaim_lib::config::runtime::set_runtime_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 初始化日志：默认 info 级输出到终端，可用 `RUST_LOG` 覆盖
/// （如 `RUST_LOG=xaim_lib=debug` 排查后端问题）。
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .compact()
        .init();
}
