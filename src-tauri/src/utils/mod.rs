//! 通用工具层。

pub mod diff;
pub mod tokens;

/// 当前系统时间的毫秒级 Unix 时间戳——全仓统一的时间戳口径。
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
