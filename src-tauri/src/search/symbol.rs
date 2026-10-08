//! 符号检索：按名字查 `symbols` 表（tree-sitter 索引的产物），供「跳转到
//! 符号 / 按名字找代码」使用。
//!
//! 两档入口，均为 ASCII 大小写不敏感、按相关度排序、最多返回 50 条：
//!
//! - [`search_symbols`]：精确 + 前缀两档，严格检索，适合交互式跳转；
//! - [`search_symbols_fuzzy`]：在两档之上追加「包含」档，宽松检索。
//!
//! 相关度从高到低：精确同名 > 前缀（名字更短者优先）> 包含（名字更短者
//! 优先）；同分按 (名字, 文件, 起始行) 决出确定性顺序。
//!
//! 索引加速：精确档 `name = ? COLLATE NOCASE` 与前缀档
//! `name COLLATE NOCASE LIKE 'q%'` 都命中 `idx_symbols_name_nocase` 索引
//! （见 schema V2 的说明）；「包含」档的 `%q%` 无法走 B-tree，只能扫描，
//! 但中型项目的符号量级（数千到数万）下实测远低于 100ms 预算。
//! 查询里的 `%`/`_`/`\` 会转义，只按字面量匹配，不会被当成通配符。

use std::collections::HashSet;

use rusqlite::{params, Connection, Row};

use crate::errors::Result;
use crate::index::symbol::Symbol;
use crate::store::Store;

/// 返回结果条数上限。
const MAX_RESULTS: usize = 50;

/// 精确档：`name = ?2 COLLATE NOCASE`，命中 idx_symbols_name_nocase 索引。
const EXACT_SQL: &str = "
    SELECT s.name, s.kind, f.path, s.start_line, s.end_line
    FROM symbols s
    JOIN files f ON s.file_id = f.id
    WHERE f.project_id = ?1 AND s.name = ?2 COLLATE NOCASE
    ORDER BY f.path, s.start_line, s.kind
    LIMIT 50";

/// 前缀/包含共用的 LIKE 档：模式由调用方拼（`q%` 或 `%q%`）。
/// LHS 显式 COLLATE NOCASE 使前缀模式能命中 idx_symbols_name_nocase 索引。
const LIKE_SQL: &str = "
    SELECT s.name, s.kind, f.path, s.start_line, s.end_line
    FROM symbols s
    JOIN files f ON s.file_id = f.id
    WHERE f.project_id = ?1 AND s.name COLLATE NOCASE LIKE ?2 ESCAPE '\\'
    ORDER BY LENGTH(s.name), s.name, f.path, s.start_line
    LIMIT 50";

/// 严格检索：精确匹配优先，其次前缀匹配。
pub fn search_symbols(store: &Store, project_id: i64, query: &str) -> Result<Vec<Symbol>> {
    let conn = store.conn();
    search_tiers(&conn, project_id, query, false)
}

/// 模糊检索：精确 > 前缀 > 包含，三档合并去重后按相关度排序。
pub fn search_symbols_fuzzy(store: &Store, project_id: i64, query: &str) -> Result<Vec<Symbol>> {
    let conn = store.conn();
    search_tiers(&conn, project_id, query, true)
}

/// 逐档查询、按档位顺序合并去重：高档命中的符号不再被低档重复计入，
/// 合并序列天然按相关度有序（各档内部已由 SQL 排好序）。
fn search_tiers(
    conn: &Connection,
    project_id: i64,
    query: &str,
    contains: bool,
) -> Result<Vec<Symbol>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let mut merged = fetch_tier(conn, EXACT_SQL, project_id, query)?;
    let pattern = format!("{}%", like_escape(query));
    merged.extend(fetch_tier(conn, LIKE_SQL, project_id, &pattern)?);
    if contains {
        let pattern = format!("%{}%", like_escape(query));
        merged.extend(fetch_tier(conn, LIKE_SQL, project_id, &pattern)?);
    }

    // 同一符号（文件内同名字同行）可能同时落进多档，保留档位最高的一次。
    let mut seen = HashSet::with_capacity(merged.len());
    merged.retain(|s| seen.insert((s.name.clone(), s.kind.clone(), s.file.clone(), s.start_line)));
    merged.truncate(MAX_RESULTS);
    Ok(merged)
}

/// 执行单档查询并折叠成 [`Symbol`]。
fn fetch_tier(conn: &Connection, sql: &str, project_id: i64, pattern: &str) -> Result<Vec<Symbol>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![project_id, pattern], row_to_symbol)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn row_to_symbol(row: &Row<'_>) -> rusqlite::Result<Symbol> {
    Ok(Symbol {
        name: row.get(0)?,
        kind: row.get(1)?,
        file: row.get(2)?,
        start_line: to_u32(row.get(3)?),
        end_line: to_u32(row.get(4)?),
    })
}

/// LIKE 模式转义：`\`、`%`、`_` 前加 `\`，配合 `ESCAPE '\'` 使用，
/// 使用户输入中的通配符按字面量匹配。
fn like_escape(query: &str) -> String {
    query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 行号以 i64 存储，折叠回 u32（异常大值钳到 u32::MAX，与索引侧一致）。
fn to_u32(line: i64) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use tempfile::tempdir;

    use crate::store::{file, project};

    /// 建库 + 注册项目，返回 (store, project_id)。
    fn setup() -> (Store, i64) {
        let root = tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let project_id = {
            let conn = store.conn();
            project::insert_project(&conn, &root.path().to_string_lossy(), "proj").unwrap()
        };
        (store, project_id)
    }

    /// 为项目添加一个文件及若干 (name, kind, start, end) 符号。
    fn add_file(store: &Store, project_id: i64, path: &str, symbols: &[(&str, &str, u32, u32)]) {
        let conn = store.conn();
        let file_id =
            file::upsert_file(&conn, project_id, path, Some("rust"), None, None, None).unwrap();
        for (name, kind, start, end) in symbols {
            conn.execute(
                "INSERT INTO symbols (file_id, name, kind, start_line, end_line)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![file_id, name, kind, i64::from(*start), i64::from(*end)],
            )
            .unwrap();
        }
    }

    /// 把检索结果折叠成 (name, kind) 便于断言。
    fn pairs(matches: &[Symbol]) -> Vec<(String, String)> {
        matches
            .iter()
            .map(|s| (s.name.clone(), s.kind.clone()))
            .collect()
    }

    fn names(matches: &[Symbol]) -> Vec<&str> {
        matches.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn exact_match_beats_prefix_then_contains() {
        let (store, pid) = setup();
        add_file(
            &store,
            pid,
            "src/service.rs",
            &[
                ("SearchService", "class", 1, 20),      // 前缀（且更短）
                ("search", "function", 22, 24),         // 精确
                ("search_symbols", "function", 26, 30), // 前缀
                ("my_searcher", "function", 32, 34),    // 仅包含
            ],
        );

        let fuzzy = search_symbols_fuzzy(&store, pid, "search").unwrap();
        assert_eq!(
            names(&fuzzy),
            vec!["search", "SearchService", "search_symbols", "my_searcher"],
            "相关度应为 精确 > 前缀（短者优先）> 包含"
        );

        // 严格模式不含「包含」档。
        let strict = search_symbols(&store, pid, "search").unwrap();
        assert_eq!(
            names(&strict),
            vec!["search", "SearchService", "search_symbols"]
        );
    }

    #[test]
    fn finds_functions_classes_and_methods() {
        let (store, pid) = setup();
        add_file(
            &store,
            pid,
            "src/app.rs",
            &[
                ("Alpha", "class", 1, 40),
                ("alpha", "function", 42, 44),
                ("run", "method", 5, 9),
                ("unrelated", "variable", 46, 46),
            ],
        );

        let alpha = search_symbols(&store, pid, "alpha").unwrap();
        assert_eq!(
            pairs(&alpha),
            vec![
                ("Alpha".to_string(), "class".to_string()),
                ("alpha".to_string(), "function".to_string()),
            ]
        );

        let run = search_symbols_fuzzy(&store, pid, "run").unwrap();
        assert_eq!(pairs(&run), vec![("run".to_string(), "method".to_string())]);
    }

    #[test]
    fn matching_is_case_insensitive_for_ascii() {
        let (store, pid) = setup();
        add_file(&store, pid, "src/grep.rs", &[("GrepMatch", "class", 1, 30)]);

        for query in ["grepmatch", "GREPMATCH", "GrepMatch", "grepMatch"] {
            let matches = search_symbols(&store, pid, query).unwrap();
            assert_eq!(names(&matches), vec!["GrepMatch"], "查询 {query} 应命中");
        }
    }

    #[test]
    fn like_wildcards_in_query_match_literally() {
        let (store, pid) = setup();
        add_file(
            &store,
            pid,
            "src/wild.rs",
            &[
                ("a_b", "function", 1, 2),
                ("axb", "function", 3, 4),
                ("100%", "variable", 5, 5),
                ("100x", "variable", 6, 6),
            ],
        );

        // 未转义时 `a_b` 的 `_` 会通配 axb，`%` 会通配一切。
        assert_eq!(
            names(&search_symbols(&store, pid, "a_b").unwrap()),
            vec!["a_b"]
        );
        assert_eq!(
            names(&search_symbols_fuzzy(&store, pid, "a_b").unwrap()),
            vec!["a_b"]
        );
        assert_eq!(
            names(&search_symbols_fuzzy(&store, pid, "100%").unwrap()),
            vec!["100%"]
        );
        assert_eq!(
            names(&search_symbols_fuzzy(&store, pid, "100x").unwrap()),
            vec!["100x"]
        );
    }

    #[test]
    fn same_name_in_multiple_files_all_returned_by_path_order() {
        let (store, pid) = setup();
        add_file(&store, pid, "src/b.rs", &[("new", "method", 3, 5)]);
        add_file(&store, pid, "src/a.rs", &[("new", "method", 10, 12)]);
        add_file(&store, pid, "src/c.rs", &[("new", "method", 1, 2)]);

        let matches = search_symbols(&store, pid, "new").unwrap();
        assert_eq!(
            matches.iter().map(|s| s.file.as_str()).collect::<Vec<_>>(),
            vec!["src/a.rs", "src/b.rs", "src/c.rs"]
        );
        assert_eq!(matches[0].start_line, 10);
    }

    #[test]
    fn results_are_scoped_to_project() {
        let (store, pid) = setup();
        let (other, other_pid) = setup();
        add_file(&store, pid, "src/main.rs", &[("alpha", "function", 1, 2)]);
        add_file(
            &other,
            other_pid,
            "src/main.rs",
            &[("alpha", "function", 1, 2)],
        );

        assert_eq!(search_symbols(&store, pid, "alpha").unwrap().len(), 1);
        assert_eq!(search_symbols(&other, other_pid, "alpha").unwrap().len(), 1);
        assert!(search_symbols(&store, 999, "alpha").unwrap().is_empty());
    }

    #[test]
    fn result_is_capped_at_50() {
        let (store, pid) = setup();
        let symbols: Vec<(String, String, u32, u32)> = (0..60)
            .map(|i| (format!("fn_{i:02}"), "function".to_string(), i + 1, i + 1))
            .collect();
        let refs: Vec<(&str, &str, u32, u32)> = symbols
            .iter()
            .map(|(n, k, s, e)| (n.as_str(), k.as_str(), *s, *e))
            .collect();
        add_file(&store, pid, "src/many.rs", &refs);

        let matches = search_symbols(&store, pid, "fn_").unwrap();
        assert_eq!(matches.len(), 50, "结果应截断到 50 条");
        assert_eq!(matches[0].name, "fn_00", "同长度按名字排序");
    }

    #[test]
    fn empty_query_returns_empty() {
        let (store, pid) = setup();
        add_file(&store, pid, "src/a.rs", &[("alpha", "function", 1, 2)]);

        assert!(search_symbols(&store, pid, "").unwrap().is_empty());
        assert!(search_symbols(&store, pid, "   ").unwrap().is_empty());
        assert!(search_symbols_fuzzy(&store, pid, "").unwrap().is_empty());
    }

    #[test]
    fn file_and_lines_are_filled_from_index() {
        let (store, pid) = setup();
        add_file(&store, pid, "src/app.rs", &[("alpha", "function", 7, 9)]);

        let s = &search_symbols(&store, pid, "alpha").unwrap()[0];
        assert_eq!(s.file, "src/app.rs");
        assert_eq!((s.start_line, s.end_line), (7, 9));
    }

    /// 用 EXPLAIN QUERY PLAN 验证精确/前缀两档走 NOCASE 名字索引（索引加速）。
    /// 模式以字面量内联：绑定参数的 LIKE 优化在运行期才决策，EQP 看不到。
    #[test]
    fn exact_and_prefix_tiers_use_the_nocase_name_index() {
        let store = Store::open_in_memory().unwrap();
        let conn = store.conn();
        for (sql, index_name) in [
            (
                "SELECT s.name FROM symbols s WHERE s.name = 'x' COLLATE NOCASE",
                "idx_symbols_name_nocase",
            ),
            (
                "SELECT s.name FROM symbols s WHERE s.name COLLATE NOCASE LIKE 'x%'",
                "idx_symbols_name_nocase",
            ),
        ] {
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let plan: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .map(|d| d.unwrap())
                .collect();
            let plan = plan.join("; ");
            assert!(
                plan.contains(index_name),
                "查询计划应使用 {index_name}，实际为 {plan}"
            );
        }
    }

    #[test]
    fn fuzzy_search_on_medium_index_under_100ms() {
        let (store, pid) = setup();
        // 200 文件 × 100 符号 = 2 万符号（中型项目量级）：一半是前缀命中，
        // 一半把关键词埋在名字中段（只能靠包含档捞出）。
        for f in 0..200u32 {
            let symbols: Vec<(String, String, u32, u32)> = (0..100u32)
                .map(|i| {
                    let name = if i % 2 == 0 {
                        format!("sym_{f:03}_{i:03}")
                    } else {
                        format!("fn_{f:03}_sym_{i:03}")
                    };
                    (name, "function".to_string(), i + 1, i + 1)
                })
                .collect();
            let refs: Vec<(&str, &str, u32, u32)> = symbols
                .iter()
                .map(|(n, k, s, e)| (n.as_str(), k.as_str(), *s, *e))
                .collect();
            add_file(&store, pid, &format!("mod_{f:03}/lib.rs"), &refs);
        }

        let start = Instant::now();
        let matches = search_symbols_fuzzy(&store, pid, "sym").unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(100),
            "2 万符号模糊检索耗时 {elapsed:?}，应 < 100ms"
        );
        println!("2 万符号模糊检索耗时 {elapsed:?}");
        assert_eq!(matches.len(), 50);
        assert_eq!(matches[0].name, "sym_000_000");
    }
}
