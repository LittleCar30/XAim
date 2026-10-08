// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! XAim 桌面端入口：初始化日志，构建 Tauri 应用并注册全部 IPC 命令。
//!
//! 命令实现见库 crate（`xaim_lib`）的 `commands` 模块；全局状态
//! [`AppState`] 持有 SQLite 存储与当前项目/任务编排器。

use xaim_lib::commands;
use xaim_lib::state::AppState;

fn main() {
    init_logging();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::project::open_project,
            commands::project::close_project,
            commands::project::list_projects,
            commands::project::get_file_tree,
            commands::project::read_file,
            commands::task::run_task,
            commands::task::cancel_task,
            commands::task::get_task,
            commands::task::list_tasks,
            commands::task::list_task_steps,
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
