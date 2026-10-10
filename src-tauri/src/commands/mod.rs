//! Tauri 命令层：薄封装，转发到各领域模块。TODO: 按域补齐命令，保持无业务逻辑。

pub mod config;
pub mod data;
pub mod model;
pub mod project;
pub mod task;

#[cfg(test)]
mod e2e;
