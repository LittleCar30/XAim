//! 上下文组装：把排序后的相关文件裁剪成 token 预算内、面向模型的代码片段。
//!
//! 按 `ranked` 顺序逐文件处理，预算耗尽即止（后面的文件相关性只会更低）。
//! 单文件的片段来源与优先级：
//!
//! 1. **import 行**（语言无关前缀启发式 + 跨行括号续行），有独立 token 上限；
//! 2. **名字命中任务关键词的符号**（函数/方法/类型，tree-sitter 提取）；
//! 3. **类型定义**（class/interface/type），规格要求即使名字不命中也带上；
//!
//! 未命中关键词的其余函数**不占预算**——片段聚焦是「优先函数级、不要整文
//! 件」的关键：单文件成本小，预算才能覆盖更多相关文件。没有 import 也没有
//! 命中片段的文件（markdown、无匹配的代码）回退为按行截取**文件头部**。
//!
//! 组装按**行掩码并集**进行：选中片段的行区间与 import 行取并集后按原文件
//! 顺序输出——嵌套符号（类与其方法同时命中）不会重复输出同一行（去重）。
//!
//! 预算控制：单文件上限 [`MAX_FILE_TOKENS`]，全局不超过 `budget_tokens`；
//! 超出时按**整项丢弃**（函数级截断，绝不在函数中间切断），从最低优先级、
//! 最靠后的项开始回退。token 估算复用 [`crate::utils::tokens::estimate_tokens`]。
//! `reason` 原样来自排序结果的 reasons，保证上下文可解释。

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::errors::Result;
use crate::index::lang::detect_language;
use crate::index::symbol::{extract_symbols, Symbol};
use crate::search::rank::{task_keywords, RankedFile};
use crate::utils::tokens;

/// 单文件上下文的 token 上限。
pub const MAX_FILE_TOKENS: usize = 3000;

/// import 块的 token 上限，防止超大 import 段吃掉文件预算。
const MAX_IMPORT_TOKENS: usize = 300;

/// 跨行 import 的续行数上限（括号失衡时的保护）。
const MAX_IMPORT_CONTINUATION: usize = 20;

/// 识别为 import 的行前缀（trim 后匹配），覆盖主流语言。
const IMPORT_PREFIXES: &[&str] = &[
    "import ",
    "import(",
    "import\t",     // ts/js/java/python…
    "from ",        // python
    "use ",         // rust
    "using ",       // c#
    "#include",     // c/cpp
    "extern crate", // rust 旧式
    "require(",
    "require ", // js (CommonJS)
    "package ", // go/java 包声明
];

/// 组装好的模型上下文。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    /// 按相关度（即 `ranked` 顺序）排列的文件片段。
    pub files: Vec<ContextFile>,
    /// 全部片段的 token 总和，恒不超过 `budget_tokens`。
    pub total_tokens: usize,
}

/// 单文件的上下文片段。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextFile {
    /// 相对项目根、以 `/` 分隔的文件路径。
    pub path: String,
    /// 裁剪后的片段（import + 相关函数/类型，或文件头部）。
    pub content: String,
    /// 命中原因（来自排序结果的 reasons），保证上下文可解释。
    pub reason: String,
    /// 本片段的 token 估算值。
    pub tokens: usize,
}

/// 依据排序结果在 token 预算内组装模型上下文，见模块文档。
pub fn build_context(
    task: &str,
    ranked: Vec<RankedFile>,
    root: &Path,
    budget_tokens: usize,
) -> Result<Context> {
    let keywords = task_keywords(task);
    let mut files: Vec<ContextFile> = Vec::new();
    let mut used = 0usize;
    let mut seen: HashSet<&str> = HashSet::new();

    for entry in &ranked {
        if used >= budget_tokens {
            break; // 预算耗尽；ranked 有序，余下文件相关性更低
        }
        if !seen.insert(entry.path.as_str()) {
            continue; // 排序结果重复出现同一路径：跳过
        }
        if !is_safe_rel(&entry.path) {
            tracing::warn!(path = %entry.path, "上下文组装拒绝越界路径");
            continue;
        }
        let Ok(content) = fs::read_to_string(root.join(&entry.path)) else {
            tracing::debug!(path = %entry.path, "上下文组装跳过不可读文件");
            continue;
        };

        let cap = MAX_FILE_TOKENS.min(budget_tokens - used);
        let Some((snippet, file_tokens)) = snippet_for(&entry.path, &content, &keywords, cap)
        else {
            continue;
        };

        used += file_tokens;
        files.push(ContextFile {
            path: entry.path.clone(),
            content: snippet,
            reason: entry.reasons.join("; "),
            tokens: file_tokens,
        });
    }

    Ok(Context {
        files,
        total_tokens: used,
    })
}

/// 只接受项目内的普通相对路径：拒绝空路径、`/` 开头、盘符前缀与 `..` 越
/// 界组件（`root.join` 遇到它们会逃出项目根）。
fn is_safe_rel(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    let p = Path::new(path);
    p.is_relative()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// 为单个文件裁剪片段，返回 (片段, token 数)；产不出内容返回 `None`。
fn snippet_for(
    path: &str,
    content: &str,
    keywords: &[String],
    cap: usize,
) -> Option<(String, usize)> {
    if cap == 0 {
        return None;
    }
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }

    // import 行（含跨行续行），按独立上限裁剪。
    let mut import_mask = vec![false; lines.len()];
    mark_import_lines(&lines, &mut import_mask);
    trim_mask(&lines, &mut import_mask, MAX_IMPORT_TOKENS.min(cap));
    let import_tokens = tokens::estimate_tokens(&join_masked(&lines, &import_mask));

    // 片段项：0 = 名字命中关键词，1 = 类型定义；未命中函数不占预算。
    let lang = detect_language(Path::new(path));
    let symbols = extract_symbols(Path::new(path), &lang, content).unwrap_or_default();
    let mut items: Vec<Item> = collect_items(&symbols, &lines, keywords);
    items.sort_by_key(|it| (it.priority, it.start));

    let item_budget = cap.saturating_sub(import_tokens);
    let mut selected: Vec<Item> = Vec::new();
    let mut used = 0usize;
    for item in items {
        if used + item.tokens <= item_budget {
            used += item.tokens;
            selected.push(item);
        }
    }

    if selected.is_empty() && import_tokens == 0 {
        // 无 import、无命中片段（文本文件 / 无匹配代码文件）：回退文件头。
        return head_snippet(&lines, cap);
    }

    // 组装（行掩码并集，嵌套/重叠行天然去重），超预算时整项回退。
    loop {
        let content = assemble(&lines, &import_mask, &selected);
        let n = tokens::estimate_tokens(&content);
        if n > 0 && n <= cap {
            return Some((content, n));
        }
        if n == 0 {
            return head_snippet(&lines, cap);
        }
        let Some(item) = selected.pop() else {
            // 只剩 import 仍超预算（import 已 ≤ cap，理论不可达）：按行截尾兜底。
            trim_mask(&lines, &mut import_mask, cap);
            let content = join_masked(&lines, &import_mask);
            let n = tokens::estimate_tokens(&content);
            return if n == 0 { None } else { Some((content, n)) };
        };
        tracing::trace!(path, item_start = item.start, "上下文片段超预算，整项丢弃");
    }
}

/// 可选片段项：符号的行区间与优先级（0 = 名字命中，1 = 类型定义）。
struct Item {
    start: usize, // 0 起，含
    end: usize,   // 0 起，不含
    priority: u8,
    tokens: usize,
}

/// 把符号折叠成行区间片段项；未命中关键词且非类型定义的符号返回 `None`。
fn collect_items(symbols: &[Symbol], lines: &[&str], keywords: &[String]) -> Vec<Item> {
    symbols
        .iter()
        .filter_map(|s| {
            let start = (s.start_line as usize).saturating_sub(1).min(lines.len());
            let end = (s.end_line as usize).min(lines.len()).max(start);
            if end <= start {
                return None;
            }
            let name_lc = s.name.to_lowercase();
            let priority = if keywords.iter().any(|kw| name_lc.contains(kw)) {
                0u8
            } else if matches!(s.kind.as_str(), "class" | "interface" | "type") {
                1
            } else {
                return None;
            };
            let text = lines[start..end].join("\n");
            Some(Item {
                start,
                end,
                priority,
                tokens: tokens::estimate_tokens(&text),
            })
        })
        .collect()
}

/// 标记 import 行：命中前缀即标记；括号未闭合（TS/Rust 多行 import）则
/// 续标后续行直至闭合，续行数有上限保护。
fn mark_import_lines(lines: &[&str], mask: &mut [bool]) {
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if is_import_line(trimmed) {
            mask[i] = true;
            let mut balance = bracket_balance(trimmed);
            let mut j = i + 1;
            while balance > 0 && j < lines.len() && j - i <= MAX_IMPORT_CONTINUATION {
                mask[j] = true;
                balance += bracket_balance(lines[j]);
                j += 1;
            }
            i = j;
        } else {
            i += 1;
        }
    }
}

fn is_import_line(trimmed: &str) -> bool {
    IMPORT_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

/// 一行的括号净增量，用于判断 import 是否跨行。
fn bracket_balance(line: &str) -> i32 {
    line.chars()
        .map(|c| match c {
            '{' | '(' | '[' => 1,
            '}' | ')' | ']' => -1,
            _ => 0,
        })
        .sum()
}

/// 从掩码尾部逐行取消标记，直到标记行的 token 数 ≤ budget。
fn trim_mask(lines: &[&str], mask: &mut [bool], budget: usize) {
    while tokens::estimate_tokens(&join_masked(lines, mask)) > budget {
        match mask.iter().rposition(|m| *m) {
            Some(last) => mask[last] = false,
            None => break,
        }
    }
}

/// 掩码行按原文件顺序拼接。
fn join_masked(lines: &[&str], mask: &[bool]) -> String {
    lines
        .iter()
        .zip(mask)
        .filter(|(_, marked)| **marked)
        .map(|(line, _)| *line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// import 行与选中片段项的行区间取并集后按原文件顺序输出。
fn assemble(lines: &[&str], import_mask: &[bool], selected: &[Item]) -> String {
    let mut mask = import_mask.to_vec();
    for item in selected {
        for slot in mask.iter_mut().take(item.end).skip(item.start) {
            *slot = true;
        }
    }
    join_masked(lines, &mask)
}

/// 无 import、无命中片段时的回退：从文件头按行截取，不超过 `cap` token。
fn head_snippet(lines: &[&str], cap: usize) -> Option<(String, usize)> {
    let mut end = 0usize;
    let mut used = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let cost = tokens::estimate_tokens(line) + 1; // + 换行
        if used + cost > cap {
            break;
        }
        used += cost;
        end = i + 1;
    }
    if end == 0 {
        return None;
    }
    let content = lines[..end].join("\n");
    let n = tokens::estimate_tokens(&content);
    Some((content, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_empty_and_oversized_first_line_do_not_consume_budget() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("binary.txt"), [255, 254]).unwrap();
        fs::write(root.path().join("empty.txt"), "").unwrap();
        fs::write(root.path().join("oversized.txt"), "x".repeat(1000)).unwrap();
        fs::write(root.path().join("valid.txt"), "hello\n").unwrap();
        let ranked = ["binary.txt", "empty.txt", "oversized.txt", "valid.txt"]
            .into_iter()
            .map(|path| entry(path, 1.0, &["match"]))
            .collect();
        let context = build_context("hello", ranked, root.path(), 10).unwrap();
        assert_eq!(paths_of(&context), vec!["valid.txt"]);
        assert_eq!(context.files[0].content, "hello");
        assert_eq!(context.total_tokens, tokens::estimate_tokens("hello"));
    }

    #[test]
    fn multiline_import_stops_after_closing_bracket_or_continuation_limit() {
        let lines = [
            "use crate::{",
            "    alpha,",
            "    beta,",
            "};",
            "fn unrelated() {}",
        ];
        let mut mask = vec![false; lines.len()];
        mark_import_lines(&lines, &mut mask);
        assert_eq!(mask, vec![true, true, true, true, false]);
        let mut unclosed = vec!["import {"; MAX_IMPORT_CONTINUATION + 3];
        unclosed[1..].fill("    name,");
        let mut mask = vec![false; unclosed.len()];
        mark_import_lines(&unclosed, &mut mask);
        assert_eq!(
            mask.iter().filter(|marked| **marked).count(),
            MAX_IMPORT_CONTINUATION + 1
        );
        assert!(!mask[MAX_IMPORT_CONTINUATION + 1]);
    }
    use tempfile::tempdir;

    /// 构造排序结果项。
    fn entry(path: &str, score: f64, reasons: &[&str]) -> RankedFile {
        RankedFile {
            path: path.to_string(),
            score,
            reasons: reasons.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// 生成一个约 `body_lines` 行的函数，行宽稳定，token 量随行数线性增长。
    fn big_fn(name: &str, body_lines: usize) -> String {
        let mut src = format!("pub fn {name}() -> u64 {{\n");
        for j in 0..body_lines {
            src.push_str(&format!(
                "    let intermediate_value_{j} = compute_step_with_long_name({j}) + {j} * 7;\n"
            ));
        }
        src.push_str("}\n");
        src
    }

    fn paths_of(context: &Context) -> Vec<&str> {
        context.files.iter().map(|f| f.path.as_str()).collect()
    }

    #[test]
    fn small_file_keeps_imports_types_and_reason() {
        let tmp = tempdir().unwrap();
        write_file(
            tmp.path(),
            "src/app.rs",
            "use std::collections::HashMap;\n\npub struct Cfg {\n    pub name: String,\n}\n\npub fn run(cfg: &Cfg) -> String {\n    format!(\"{}\", cfg.name)\n}\n",
        );
        let ranked = vec![entry(
            "src/app.rs",
            1.0,
            &["grep 命中 ×1", "符号匹配 ×1: run"],
        )];

        let context = build_context("run", ranked, tmp.path(), 10_000).unwrap();

        assert_eq!(context.files.len(), 1);
        let file = &context.files[0];
        assert_eq!(file.path, "src/app.rs");
        assert_eq!(file.reason, "grep 命中 ×1; 符号匹配 ×1: run");
        assert!(file.content.contains("use std::collections::HashMap;"));
        assert!(file.content.contains("pub struct Cfg {"));
        assert!(file.content.contains("pub fn run"));
        assert_eq!(file.tokens, tokens::estimate_tokens(&file.content));
        assert_eq!(context.total_tokens, file.tokens);
    }

    #[test]
    fn big_file_takes_matched_function_not_whole() {
        let tmp = tempdir().unwrap();
        let mut src = String::from("use crate::worker;\n\n");
        for i in 0..12 {
            src.push_str(&big_fn(&format!("alpha_{i:02}"), 30));
            src.push('\n');
        }
        src.push_str(&big_fn("rank_task_fn", 30));
        write_file(tmp.path(), "src/huge.rs", &src);
        let whole_tokens = tokens::estimate_tokens(&src);
        assert!(
            whole_tokens > MAX_FILE_TOKENS,
            "测试前置：整文件应超过单文件上限"
        );

        let ranked = vec![entry("src/huge.rs", 2.0, &["符号匹配 ×1: rank_task_fn"])];
        let context = build_context("rank", ranked, tmp.path(), 50_000).unwrap();

        assert_eq!(context.files.len(), 1);
        let file = &context.files[0];
        assert!(file.content.contains("fn rank_task_fn"), "命中的函数应入选");
        assert!(
            !file.content.contains("fn alpha_00"),
            "未命中的函数不应为凑满预算而铺满片段"
        );
        assert!(file.tokens <= MAX_FILE_TOKENS);
        assert!(
            file.tokens * 2 < whole_tokens,
            "片段应显著小于整文件（{} < {} 的一半）",
            file.tokens,
            whole_tokens
        );
        assert_eq!(
            file.content.lines().last(),
            Some("}"),
            "截断只发生在函数边界"
        );
    }

    #[test]
    fn many_matched_functions_truncate_at_function_boundary() {
        let tmp = tempdir().unwrap();
        let mut src = String::new();
        for i in 0..6 {
            src.push_str(&big_fn(&format!("heavy_fn_{i}"), 60));
            src.push('\n');
        }
        write_file(tmp.path(), "src/heavy.rs", &src);

        let ranked = vec![entry("src/heavy.rs", 1.0, &["grep 命中 ×6"])];
        let context = build_context("heavy_fn", ranked, tmp.path(), 50_000).unwrap();

        let file = &context.files[0];
        assert!(file.tokens <= MAX_FILE_TOKENS, "单文件不得超过 3000 token");
        assert!(
            file.content.contains("fn heavy_fn_0"),
            "最靠前的命中函数应保留"
        );
        assert!(
            !file.content.contains("fn heavy_fn_5"),
            "放不下的函数应整项丢弃"
        );
        assert_eq!(file.content.lines().last(), Some("}"), "不应在函数中间截断");
    }

    #[test]
    fn global_budget_consumed_in_ranked_order() {
        let tmp = tempdir().unwrap();
        for name in ["a", "b", "c"] {
            write_file(
                tmp.path(),
                &format!("src/{name}.rs"),
                &format!("pub fn target_{name}() -> u64 {{\n    {name}.len() as u64\n}}\n"),
            );
        }
        let ranked = vec![
            entry("src/a.rs", 3.0, &["grep 命中 ×1"]),
            entry("src/b.rs", 2.0, &["grep 命中 ×1"]),
            entry("src/c.rs", 1.0, &["grep 命中 ×1"]),
        ];

        // 先用大预算测出每文件片段的 token 数，再据此设紧预算。
        let generous = build_context("target", ranked.clone(), tmp.path(), 100_000).unwrap();
        assert_eq!(generous.files.len(), 3);
        let budget = generous.files[0].tokens + generous.files[1].tokens;

        let tight = build_context("target", ranked, tmp.path(), budget).unwrap();

        assert_eq!(
            paths_of(&tight),
            vec!["src/a.rs", "src/b.rs"],
            "按 ranked 顺序消费预算"
        );
        assert_eq!(tight.total_tokens, budget, "总 token 不超过预算");
    }

    #[test]
    fn imports_and_type_defs_kept_without_keyword_match() {
        let tmp = tempdir().unwrap();
        write_file(
            tmp.path(),
            "src/cfg.rs",
            "use std::fmt;\n\npub struct UnrelatedCfg {\n    pub size: u32,\n}\n\npub fn totally_different() -> u32 {\n    1\n}\n",
        );
        let ranked = vec![entry("src/cfg.rs", 1.0, &["grep 命中 ×1"])];

        let context = build_context("zzz", ranked, tmp.path(), 10_000).unwrap();

        let file = &context.files[0];
        assert!(file.content.contains("use std::fmt;"), "import 应始终保留");
        assert!(
            file.content.contains("pub struct UnrelatedCfg {"),
            "类型定义即使名字不命中也应保留"
        );
        assert!(
            !file.content.contains("fn totally_different"),
            "未命中的函数不占预算"
        );
    }

    #[test]
    fn nested_symbols_emit_lines_once() {
        let tmp = tempdir().unwrap();
        let src = "export class Widget {\n    name: string = \"\";\n\n    mount(): string {\n        return this.name;\n    }\n}\n";
        write_file(tmp.path(), "src/widget.ts", src);
        let ranked = vec![entry("src/widget.ts", 1.0, &["符号匹配 ×2"])];

        let context = build_context("widget mount", ranked, tmp.path(), 10_000).unwrap();

        let file = &context.files[0];
        // 类与嵌套方法同时命中，重叠行只应输出一次。
        assert_eq!(
            file.content.matches("return this.name;").count(),
            1,
            "嵌套片段去重"
        );
        assert_eq!(
            file.content,
            src.trim_end(),
            "类覆盖全部行，片段应与原文件一致"
        );
    }

    #[test]
    fn duplicate_paths_and_missing_files_are_skipped() {
        let tmp = tempdir().unwrap();
        write_file(tmp.path(), "src/real.rs", "pub fn target_fn() {}\n");
        let ranked = vec![
            entry("src/ghost.rs", 3.0, &["grep 命中 ×1"]),
            entry("src/real.rs", 2.0, &["grep 命中 ×1"]),
            entry("src/real.rs", 2.0, &["grep 命中 ×1"]),
        ];

        let context = build_context("target", ranked, tmp.path(), 10_000).unwrap();

        assert_eq!(
            paths_of(&context),
            vec!["src/real.rs"],
            "缺失文件跳过、重复路径去重"
        );
    }

    #[test]
    fn path_escape_attempts_are_rejected() {
        let tmp = tempdir().unwrap();
        write_file(tmp.path(), "src/ok.rs", "pub fn target_fn() {}\n");
        let ranked = vec![
            entry("../evil.rs", 3.0, &["grep 命中 ×1"]),
            entry("/abs/target.rs", 2.0, &["grep 命中 ×1"]),
            entry("src/ok.rs", 1.0, &["grep 命中 ×1"]),
        ];

        let context = build_context("target", ranked, tmp.path(), 10_000).unwrap();

        assert_eq!(paths_of(&context), vec!["src/ok.rs"], "越界路径不应被读取");
    }

    #[test]
    fn text_file_falls_back_to_head() {
        let tmp = tempdir().unwrap();
        let small = (0..10)
            .map(|i| format!("第 {i} 段设计说明。"))
            .collect::<Vec<_>>()
            .join("\n");
        write_file(tmp.path(), "docs/small.md", &small);
        let big: String = (0..400)
            .map(|i| format!("design note line {i} with some padding text here"))
            .collect::<Vec<_>>()
            .join("\n");
        write_file(tmp.path(), "docs/big.md", &big);
        let ranked = vec![
            entry("docs/small.md", 1.0, &["grep 命中 ×1"]),
            entry("docs/big.md", 1.0, &["grep 命中 ×1"]),
        ];

        let context = build_context("设计 说明", ranked, tmp.path(), 50_000).unwrap();

        let small_file = &context.files[0];
        assert_eq!(small_file.content, small, "小文本文件整体保留");
        let big_file = &context.files[1];
        assert!(big_file.tokens <= MAX_FILE_TOKENS);
        assert!(
            big_file.content.lines().count() < 400,
            "大文本文件应按行截取头部"
        );
        assert!(
            big_file.content.starts_with("design note line 0"),
            "头部回退应从文件开头开始"
        );
    }

    #[test]
    fn zero_budget_and_empty_ranked_yield_empty() {
        let tmp = tempdir().unwrap();
        write_file(tmp.path(), "src/a.rs", "pub fn target_fn() {}\n");
        let ranked = vec![entry("src/a.rs", 1.0, &["grep 命中 ×1"])];

        let zero = build_context("target", ranked.clone(), tmp.path(), 0).unwrap();
        assert!(zero.files.is_empty());
        assert_eq!(zero.total_tokens, 0);

        let none = build_context("target", Vec::new(), tmp.path(), 10_000).unwrap();
        assert!(none.files.is_empty());
    }

    #[test]
    fn relevant_files_hit_rate_above_70_percent() {
        let tmp = tempdir().unwrap();
        let mut ranked = Vec::new();
        // 8 个相关文件：import + 命中函数 + 若干未命中函数（整文件偏大）。
        for i in 0..8u32 {
            let mut src = format!("use crate::shared_types_{i};\n\n");
            src.push_str(&big_fn(&format!("parse_record_{i}"), 30));
            src.push('\n');
            for k in 0..3u32 {
                src.push_str(&big_fn(&format!("unrelated_helper_{i}_{k}"), 20));
                src.push('\n');
            }
            write_file(tmp.path(), &format!("src/rel_{i}.rs"), &src);
            ranked.push(entry(&format!("src/rel_{i}.rs"), 5.0, &["符号匹配 ×1"]));
        }
        // 4 个噪声文件：类型定义 + 未命中函数。
        for i in 0..4u32 {
            let mut src = format!("pub struct NoiseConfig_{i} {{\n    pub enabled: bool,\n}}\n\n");
            for k in 0..3u32 {
                src.push_str(&big_fn(&format!("noise_helper_{i}_{k}"), 20));
                src.push('\n');
            }
            write_file(tmp.path(), &format!("src/noise_{i}.rs"), &src);
            ranked.push(entry(&format!("src/noise_{i}.rs"), 1.0, &["被引用 ×1"]));
        }
        // 交错排列，模拟真实排序输出。
        ranked.rotate_right(1);

        // 预算设在「整文件策略放不下几个文件、函数级片段足够覆盖全部」的
        // 区间：整文件合计约 2 万 token，函数级片段合计约 4.6 千。
        let budget = 5_000usize;
        let context = build_context("parse record", ranked, tmp.path(), budget).unwrap();

        assert!(
            context.total_tokens <= budget,
            "总 token ({}) 不得超过预算",
            context.total_tokens
        );
        // 命中判定从严：不只要求路径出现，还要求**相关函数本体**在片段里
        // （预算耗尽时仅剩 import 的空壳不应计入命中率）。
        let relevant_in: usize = (0..8)
            .filter(|i| {
                context.files.iter().any(|f| {
                    f.path == format!("src/rel_{i}.rs")
                        && f.content.contains(&format!("fn parse_record_{i}"))
                })
            })
            .count();
        let hit_rate = relevant_in as f64 / 8.0;
        assert!(
            hit_rate > 0.7,
            "相关文件命中率 {hit_rate:.2} 应 > 70%（命中 {relevant_in}/8）"
        );
        assert_eq!(
            relevant_in, 8,
            "函数级片段成本小，预算应足够覆盖全部相关文件"
        );
        // 相关文件排在前面（保持 ranked 顺序）。
        let rel_positions: Vec<usize> = context
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.path.starts_with("src/rel_"))
            .map(|(i, _)| i)
            .collect();
        assert!(
            rel_positions.windows(2).all(|w| w[0] < w[1]),
            "上下文文件应保持 ranked 顺序"
        );
    }
}
