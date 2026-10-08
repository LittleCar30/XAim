//! 通用 KV 缓存：cache 表的读写与过期清理。
//!
//! 约定：函数全部接收 `&Connection` 而不自行加锁，由调用方经
//! `Store::conn()` 独占借用后传入；`expire_at` 为毫秒级 `i64`，
//! `NULL` 表示永不过期；过期判定为 `expire_at <= 当前时间`（含边界）。

use rusqlite::{params, Connection, OptionalExtension};

use crate::errors::Result;
use crate::utils::now_ms;

/// 读取缓存项；键不存在或已过期均返回 `None`。
///
/// 命中的过期条目会被顺带删除（惰性清理），批量清扫用 [`clear_expired`]。
pub fn get_cache(conn: &Connection, key: &str) -> Result<Option<String>> {
    let row: Option<(String, Option<i64>)> = conn
        .query_row(
            "SELECT value, expire_at FROM cache WHERE key = ?1",
            [key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let Some((value, expire_at)) = row else {
        return Ok(None);
    };
    match expire_at {
        None => Ok(Some(value)),
        Some(deadline) if deadline > now_ms() => Ok(Some(value)),
        // 已过期：视为不存在，顺带清除该行。
        Some(_) => {
            conn.execute("DELETE FROM cache WHERE key = ?1", [key])?;
            Ok(None)
        }
    }
}

/// 写入缓存项；同键覆盖。`ttl_sec` 为 `None` 表示永不过期。
pub fn set_cache(conn: &Connection, key: &str, value: &str, ttl_sec: Option<i64>) -> Result<()> {
    conn.execute(
        "INSERT INTO cache (key, value, expire_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             expire_at = excluded.expire_at",
        params![key, value, expire_at(ttl_sec)],
    )?;
    Ok(())
}

/// 批量清除全部已过期条目（不过期的不受影响）。
pub fn clear_expired(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM cache WHERE expire_at IS NOT NULL AND expire_at <= ?1",
        [now_ms()],
    )?;
    Ok(())
}

/// 删除指定缓存项；键不存在时影响 0 行、不报错。
pub fn delete_cache(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM cache WHERE key = ?1", [key])?;
    Ok(())
}

/// 把 TTL（秒）换算为过期时刻的毫秒时间戳。
fn expire_at(ttl_sec: Option<i64>) -> Option<i64> {
    ttl_sec.map(|ttl| now_ms() + ttl.saturating_mul(1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::mem_conn;

    /// 直查行数，规避 get_cache 的惰性删除干扰断言。
    fn row_count(conn: &Connection, key: &str) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM cache WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn set_then_get_roundtrip() {
        let conn = mem_conn();
        set_cache(&conn, "index:hash", "blake3:abc", None).unwrap();
        assert_eq!(
            get_cache(&conn, "index:hash").unwrap().as_deref(),
            Some("blake3:abc")
        );
        assert!(get_cache(&conn, "missing").unwrap().is_none());
    }

    #[test]
    fn set_cache_overwrites_existing_key() {
        let conn = mem_conn();
        set_cache(&conn, "k", "v1", None).unwrap();
        set_cache(&conn, "k", "v2", Some(3600)).unwrap();

        assert_eq!(get_cache(&conn, "k").unwrap().as_deref(), Some("v2"));
        // 覆盖同时刷新了 TTL：行仍存在且带过期时间。
        let expire_at: Option<i64> = conn
            .query_row("SELECT expire_at FROM cache WHERE key = 'k'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(expire_at.unwrap() > now_ms(), "覆盖应写入新的过期时刻");
    }

    #[test]
    fn ttl_none_means_never_expires() {
        let conn = mem_conn();
        set_cache(&conn, "k", "persistent", None).unwrap();

        // 即便批量清扫跑过，无 TTL 的条目仍然有效。
        clear_expired(&conn).unwrap();
        assert_eq!(
            get_cache(&conn, "k").unwrap().as_deref(),
            Some("persistent")
        );
    }

    #[test]
    fn expired_entry_is_invisible_and_lazily_removed() {
        let conn = mem_conn();
        // TTL 0 秒：expire_at 即当前时刻，按 <= 判定立即过期（确定性，无需 sleep）。
        set_cache(&conn, "k", "stale", Some(0)).unwrap();
        assert!(row_count(&conn, "k") == 1);

        assert!(get_cache(&conn, "k").unwrap().is_none(), "过期条目应不可见");
        assert_eq!(row_count(&conn, "k"), 0, "过期条目应被惰性删除");
    }

    #[test]
    fn clear_expired_removes_only_expired_entries() {
        let conn = mem_conn();
        set_cache(&conn, "expired", "a", Some(0)).unwrap();
        set_cache(&conn, "fresh", "b", Some(3600)).unwrap();
        set_cache(&conn, "forever", "c", None).unwrap();

        clear_expired(&conn).unwrap();

        assert_eq!(row_count(&conn, "expired"), 0);
        assert_eq!(get_cache(&conn, "fresh").unwrap().as_deref(), Some("b"));
        assert_eq!(get_cache(&conn, "forever").unwrap().as_deref(), Some("c"));
    }

    #[test]
    fn delete_cache_removes_row_and_ignores_missing_key() {
        let conn = mem_conn();
        set_cache(&conn, "k", "v", None).unwrap();

        delete_cache(&conn, "k").unwrap();
        assert!(get_cache(&conn, "k").unwrap().is_none());

        // 再删一次（键不存在）不报错。
        delete_cache(&conn, "k").unwrap();
    }
}
