//! XAim 桌面端 Rust 后端库：各领域模块与命令实现。
//!
//! 应用构建与命令注册在二进制入口 `main.rs`（含日志初始化），
//! 本 crate 只提供领域模块：
//! - `commands`：Tauri IPC 命令层（薄封装）；
//! - `state`：全局应用状态（SQLite 存储、当前项目、任务编排器）；
//! - 其余为各领域模块（索引/检索/编排/存储等）。

pub mod agent;
pub mod commands;
pub mod config;
pub mod data;
pub mod errors;
pub mod events;
pub mod git;
pub mod index;
pub mod model;
pub mod prompts;
pub mod repair;
pub mod search;
pub mod state;
pub mod store;
pub mod tools;
pub mod types;
pub mod utils;
pub mod verify;
