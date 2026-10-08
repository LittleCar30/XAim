//! 检索结果融合与重排：把 grep 命中、符号命中与文件索引元数据融合成
//! 按文件聚合、按相关度排序的 [`RankedFile`] 列表。
//!
//! 候选集合 = `files` ∪ grep 命中路径 ∪ 符号所在文件（按此顺序首现，
//! 排序打平时保持该顺序，保证结果稳定）。
//!
//! 原始分（越高越相关）：
//! - grep 命中：每条 +1；
//! - 符号名匹配：名字包含任务关键词的符号，每个**去重符号名** +3；
//! - 根目录文件：路径不含 `/` +0.5（项目根下的入口/配置更常被关注）；
//! - 最近修改：`mtime` 距今 7 天内 +0.5（用户正在改的文件更相关）；
//! - 被引用：+0.2 × 引用文件数。无导入图时的低成本近似——其他文件的
//!   grep 命中行文本（小写化）中出现本文件的文件名 stem（不含扩展名、
//!   大小写不敏感）即记一次引用，按去重后的引用文件数计，自身文本不算。
//!
//! 融合规则：
//! - 归一化：`score = raw / max(raw)`，落到 0-1（最高分文件恰为 1.0）；
//!   原始分为 0 的候选（与任务毫无信号）直接丢弃；
//! - 排序稳定：`sort_by` 为稳定排序，同分保持候选首现顺序；
//! - Top N：最多返回 [`TOP_N`] 条。
//!
//! 任务分词按非字母数字字符切分并小写化；纯 ASCII 的单字符词（噪声太
//! 大）忽略，中文等多字节词保留。局限：中文连写（无分隔符）无法再切分，
//! 此时符号名只能对整段做包含匹配。

use std::collections::HashMap;

use serde::Serialize;

use crate::index::symbol::Symbol;
use crate::search::grep::GrepMatch;
use crate::types::FileRecord;
use crate::utils::now_ms;

/// 返回结果条数上限。
pub const TOP_N: usize = 20;

/// 「最近修改」的判定窗口：7 天。
const RECENT_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// 各维度权重。
const GREP_SCORE: f64 = 1.0;
const SYMBOL_SCORE: f64 = 3.0;
const ROOT_SCORE: f64 = 0.5;
const RECENT_SCORE: f64 = 0.5;
const REFERENCE_SCORE: f64 = 0.2;

/// 参与引用统计的最短文件名 stem 长度：单字符 stem（如 `a.rs`）做子串
/// 匹配的噪声远大于信号。
const MIN_STEM_LEN: usize = 2;

/// 引用之外的列表型 reason 最多列出的符号名个数。
const MAX_LISTED_NAMES: usize = 3;

/// 融合重排后的单文件结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedFile {
    /// 相对项目根、以 `/` 分隔的文件路径（与各检索结果一致）。
    pub path: String,
    /// 归一化到 0-1 的相关度得分。
    pub score: f64,
    /// 命中原因（人可读，按 grep / 符号 / 根目录 / 最近修改 / 被引用顺序）。
    pub reasons: Vec<String>,
}

/// 融合 grep / 符号 / 文件元数据三路结果并重排，见模块文档。
pub fn rank_results(
    task: &str,
    grep: Vec<GrepMatch>,
    symbols: Vec<Symbol>,
    files: Vec<FileRecord>,
) -> Vec<RankedFile> {
    let keywords = task_keywords(task);

    // 候选登记：path → 首现序号。files 先行，grep / 符号补充仅在其间
    // 出现的路径（如尚未重新索引的文件）；排序打平时按该顺序稳定输出。
    let mut order: Vec<String> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut mtime_by_path: HashMap<String, Option<i64>> = HashMap::new();
    let mut admit = |path: &str, mtime: Option<i64>| {
        if seen.insert(path.to_string(), order.len()).is_none() {
            order.push(path.to_string());
            mtime_by_path.insert(path.to_string(), mtime);
        }
    };
    for record in &files {
        admit(&record.path, record.mtime);
    }

    // grep：每文件命中数 + 小写化的命中行文本（供引用统计）。
    let mut grep_count: HashMap<String, usize> = HashMap::new();
    let mut text_by_file: HashMap<String, String> = HashMap::new();
    for m in &grep {
        *grep_count.entry(m.path.clone()).or_insert(0) += 1;
        let text = text_by_file.entry(m.path.clone()).or_default();
        text.push_str(&m.text.to_lowercase());
        text.push('\n');
        admit(&m.path, None);
    }

    // 符号：名字包含任一任务关键词的，按文件归集去重后的符号名。
    let mut symbol_hits: HashMap<String, Vec<String>> = HashMap::new();
    for s in &symbols {
        let name_lc = s.name.to_lowercase();
        if keywords.iter().any(|kw| name_lc.contains(kw)) {
            let names = symbol_hits.entry(s.file.clone()).or_default();
            if !names.iter().any(|n| n == &s.name) {
                names.push(s.name.clone());
            }
        }
        admit(&s.file, None);
    }

    let now = now_ms();
    let ranked: Vec<(String, f64, Vec<String>)> = order
        .into_iter()
        .map(|path| {
            let mut raw = 0.0;
            let mut reasons = Vec::new();

            if let Some(&n) = grep_count.get(&path) {
                raw += GREP_SCORE * n as f64;
                reasons.push(format!("grep 命中 ×{n}"));
            }
            if let Some(names) = symbol_hits.get(&path) {
                raw += SYMBOL_SCORE * names.len() as f64;
                let listed = names
                    .iter()
                    .take(MAX_LISTED_NAMES)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                let more = if names.len() > MAX_LISTED_NAMES {
                    " 等"
                } else {
                    ""
                };
                reasons.push(format!("符号匹配 ×{}: {listed}{more}", names.len()));
            }
            if !path.contains('/') {
                raw += ROOT_SCORE;
                reasons.push("根目录文件".to_string());
            }
            if let Some(Some(mtime)) = mtime_by_path.get(&path) {
                if *mtime >= now - RECENT_WINDOW_MS {
                    raw += RECENT_SCORE;
                    reasons.push("7 天内修改".to_string());
                }
            }
            let stem = file_stem(&path);
            if stem.len() >= MIN_STEM_LEN {
                let refs = text_by_file
                    .iter()
                    .filter(|(g, text)| g.as_str() != path && text.contains(&stem))
                    .count();
                if refs > 0 {
                    raw += REFERENCE_SCORE * refs as f64;
                    reasons.push(format!("被引用 ×{refs}"));
                }
            }

            (path, raw, reasons)
        })
        .collect();

    // 归一化到 0-1；0 分候选与任务无任何信号，直接丢弃。
    let max = ranked
        .iter()
        .map(|(_, raw, _)| *raw)
        .fold(0.0_f64, f64::max);
    let mut result: Vec<RankedFile> = ranked
        .into_iter()
        .filter(|(_, raw, _)| *raw > 0.0)
        .map(|(path, raw, reasons)| RankedFile {
            path,
            score: if max > 0.0 { raw / max } else { 0.0 },
            reasons,
        })
        .collect();

    // sort_by 为稳定排序：同分保持候选首现顺序。
    result.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    result.truncate(TOP_N);
    result
}

/// 任务分词：按非字母数字字符切分、小写化；纯 ASCII 的单字符词忽略，
/// 其余（含中文等多字节词）保留。上下文组装（`search::context`）复用
/// 同一分词，保证「排序命中」与「片段命中」的口径一致。
pub(crate) fn task_keywords(task: &str) -> Vec<String> {
    task.split(|c: char| !c.is_alphanumeric())
        .filter(|kw| !kw.is_empty())
        .filter(|kw| kw.chars().count() >= 2 || !kw.is_ascii())
        .map(str::to_lowercase)
        .collect()
}

/// 文件名 stem：最后一段路径去掉最后一个扩展名（`a.b.c` → `a.b`）；
/// dotfile（如 `.gitignore`）去扩展后为空时退回完整文件名。
/// 返回小写形式，供大小写不敏感的引用匹配。
fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    };
    stem.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条 grep 命中（行号列号与排序无关，固定占位）。
    fn grep(path: &str, text: &str) -> GrepMatch {
        GrepMatch {
            path: path.to_string(),
            line: 1,
            column: 1,
            text: text.to_string(),
        }
    }

    fn symbol(name: &str, kind: &str, file: &str) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind: kind.to_string(),
            file: file.to_string(),
            start_line: 1,
            end_line: 2,
        }
    }

    fn file_record(path: &str, mtime: Option<i64>) -> FileRecord {
        FileRecord {
            id: 0,
            project_id: 0,
            path: path.to_string(),
            language: None,
            size: None,
            mtime,
            hash: None,
            summary: None,
            indexed_at: None,
        }
    }

    fn paths(matches: &[RankedFile]) -> Vec<&str> {
        matches.iter().map(|r| r.path.as_str()).collect()
    }

    #[test]
    fn grep_hits_and_symbol_matches_dominate() {
        let grep = vec![
            grep("src/a.rs", "let x = search();"),
            grep("src/a.rs", "search_rank();"),
            grep("src/a.rs", "do search again"),
            grep("src/b.rs", "search();"),
        ];
        let symbols = vec![
            symbol("search_engine", "class", "src/a.rs"),
            symbol("rank_helper", "function", "src/a.rs"),
            symbol("search", "function", "src/b.rs"),
        ];
        let files = vec![file_record("src/a.rs", None), file_record("src/b.rs", None)];

        let ranked = rank_results("search rank", grep, symbols, files);

        assert_eq!(paths(&ranked), vec!["src/a.rs", "src/b.rs"]);
        // raw: a = 3×1 + 2×3 = 9（最高分 → 1.0），b = 1 + 3 = 4。
        assert_eq!(ranked[0].score, 1.0);
        assert!((ranked[1].score - 4.0 / 9.0).abs() < 1e-9);
        assert_eq!(
            ranked[0].reasons,
            vec![
                "grep 命中 ×3".to_string(),
                "符号匹配 ×2: search_engine, rank_helper".to_string(),
            ]
        );
        assert_eq!(
            ranked[1].reasons,
            vec![
                "grep 命中 ×1".to_string(),
                "符号匹配 ×1: search".to_string()
            ]
        );
    }

    #[test]
    fn root_and_recent_bonuses_apply() {
        let now = now_ms();
        let month_ago = now - 30 * 24 * 60 * 60 * 1000;
        let grep = vec![
            grep("main.rs", "x"),
            grep("src/lib.rs", "x"),
            grep("src/new.rs", "x"),
            grep("top.rs", "x"), // 只出现在 grep 结果里，无文件元数据
        ];
        let files = vec![
            file_record("main.rs", Some(now)),          // 根目录 + 最近修改
            file_record("src/lib.rs", Some(month_ago)), // 都不满足
            file_record("src/new.rs", Some(now - 3_600_000)), // 仅最近修改
        ];

        let ranked = rank_results("zzz", grep, Vec::new(), files);

        // raw: main = 2.0，new = top = 1.5，lib = 1.0；new 与 top 同分，
        // 稳定排序保持首现顺序（files 先于 grep-only 的 top.rs）。
        assert_eq!(
            paths(&ranked),
            vec!["main.rs", "src/new.rs", "top.rs", "src/lib.rs"]
        );
        assert_eq!(
            ranked[0].reasons,
            vec![
                "grep 命中 ×1".to_string(),
                "根目录文件".to_string(),
                "7 天内修改".to_string()
            ]
        );
        assert_eq!(
            ranked[1].reasons,
            vec!["grep 命中 ×1".to_string(), "7 天内修改".to_string()]
        );
        assert_eq!(
            ranked[2].reasons,
            vec!["grep 命中 ×1".to_string(), "根目录文件".to_string()]
        );
        assert_eq!(ranked[3].reasons, vec!["grep 命中 ×1".to_string()]);
        assert!((ranked[1].score - 0.75).abs() < 1e-9);
        assert!((ranked[3].score - 0.5).abs() < 1e-9);
    }

    #[test]
    fn references_boost_referenced_files() {
        let grep = vec![
            grep("src/app.rs", "use crate::utils::config;"),
            grep("src/main.rs", "let c = config::load();"),
            grep("src/config.rs", "pub struct Config;"), // 自身文本不算自引用
            grep("src/other.rs", "unrelated stuff"),
        ];
        let files: Vec<FileRecord> = ["src/app.rs", "src/main.rs", "src/config.rs", "src/other.rs"]
            .iter()
            .map(|p| file_record(p, None))
            .collect();

        let ranked = rank_results("config", grep, Vec::new(), files);

        // config.rs 被另外 2 个文件的命中行提及 → +0.4，其余各 1.0 同分。
        assert_eq!(
            paths(&ranked),
            vec!["src/config.rs", "src/app.rs", "src/main.rs", "src/other.rs"]
        );
        assert_eq!(
            ranked[0].reasons,
            vec!["grep 命中 ×1".to_string(), "被引用 ×2".to_string()]
        );
        assert!((ranked[0].score - 1.0).abs() < 1e-9);
        assert!((ranked[1].score - 1.0 / 1.4).abs() < 1e-9);
    }

    #[test]
    fn references_count_files_not_lines_and_ignore_self() {
        let grep = vec![
            grep("src/g1.rs", "use util;"),
            grep("src/g1.rs", "util::helper();"), // 同一文件提及两次只计一次
            grep("src/util.rs", "util::helper();"), // 自引用忽略
            grep("src/other.rs", "nothing here"),
        ];
        let files: Vec<FileRecord> = ["src/g1.rs", "src/util.rs", "src/other.rs"]
            .iter()
            .map(|p| file_record(p, None))
            .collect();

        let ranked = rank_results("zzz", grep, Vec::new(), files);

        // raw: g1 = 2.0（两次命中），util = 1.0 + 0.2×1，other = 1.0。
        assert_eq!(
            paths(&ranked),
            vec!["src/g1.rs", "src/util.rs", "src/other.rs"]
        );
        assert_eq!(ranked[1].reasons.last().unwrap(), "被引用 ×1");
    }

    #[test]
    fn single_char_stems_are_ignored_for_references() {
        let grep = vec![
            grep("src/app.rs", "mod a; mod b;"),
            grep("src/a.rs", "nothing"),
            grep("src/b.rs", "nothing"),
        ];
        let files: Vec<FileRecord> = ["src/app.rs", "src/a.rs", "src/b.rs"]
            .iter()
            .map(|p| file_record(p, None))
            .collect();

        let ranked = rank_results("zzz", grep, Vec::new(), files);

        assert!(
            ranked
                .iter()
                .all(|r| !r.reasons.iter().any(|reason| reason.starts_with("被引用"))),
            "单字符 stem 不应参与引用统计"
        );
    }

    #[test]
    fn zero_score_candidates_are_dropped() {
        let files = vec![file_record("src/a.rs", None), file_record("src/b.rs", None)];
        assert!(rank_results("zzz", Vec::new(), Vec::new(), files.clone()).is_empty());
        assert!(rank_results("", Vec::new(), Vec::new(), files).is_empty());
    }

    #[test]
    fn top_n_limit_and_stable_ties() {
        let files: Vec<FileRecord> = (0..25)
            .map(|i| file_record(&format!("mod_{i:02}/file.rs"), None))
            .collect();
        let grep: Vec<GrepMatch> = files.iter().map(|f| grep(&f.path, "x")).collect();

        let ranked = rank_results("zzz", grep, Vec::new(), files);

        assert_eq!(ranked.len(), TOP_N, "结果应截断到 TOP_N");
        // 25 个文件同分（各 1 次命中），稳定排序应保持输入顺序取前 20。
        let expected: Vec<String> = (0..TOP_N).map(|i| format!("mod_{i:02}/file.rs")).collect();
        let got: Vec<String> = ranked.iter().map(|r| r.path.clone()).collect();
        assert_eq!(got, expected);
        assert!(ranked.iter().all(|r| (r.score - 1.0).abs() < 1e-9));
    }

    #[test]
    fn task_keywords_drive_symbol_matching() {
        let symbols = vec![
            symbol("SearchService", "class", "src/svc.rs"),
            symbol("检索服务", "class", "src/cn.rs"),
            symbol("alpha", "function", "src/single.rs"),
            symbol("fixer", "function", "src/single.rs"),
        ];
        let files: Vec<FileRecord> = ["src/svc.rs", "src/cn.rs", "src/single.rs"]
            .iter()
            .map(|p| file_record(p, None))
            .collect();

        let ranked = rank_results("SEARCH service 修复 检索 a fix", Vec::new(), symbols, files);

        // 关键词：search/service/修复/检索/fix（单字符 "a" 被忽略）。
        assert_eq!(
            paths(&ranked),
            vec!["src/svc.rs", "src/cn.rs", "src/single.rs"]
        );
        assert_eq!(
            ranked[0].reasons,
            vec!["符号匹配 ×1: SearchService".to_string()]
        );
        assert_eq!(ranked[1].reasons, vec!["符号匹配 ×1: 检索服务".to_string()]);
        // "alpha" 只被被忽略的 "a" 覆盖，不应计入。
        assert_eq!(ranked[2].reasons, vec!["符号匹配 ×1: fixer".to_string()]);
    }

    #[test]
    fn symbol_reason_lists_at_most_three_names() {
        let symbols: Vec<Symbol> = (1..=5)
            .map(|i| symbol(&format!("sym{i}"), "function", "src/many.rs"))
            .collect();
        let files = vec![file_record("src/many.rs", None)];

        let ranked = rank_results("sym", Vec::new(), symbols, files);

        assert_eq!(ranked.len(), 1);
        assert_eq!(
            ranked[0].reasons,
            vec!["符号匹配 ×5: sym1, sym2, sym3 等".to_string()]
        );
    }

    #[test]
    fn empty_inputs_yield_empty() {
        assert!(rank_results("", Vec::new(), Vec::new(), Vec::new()).is_empty());
        assert!(rank_results("fix bug", Vec::new(), Vec::new(), Vec::new()).is_empty());
    }
}
