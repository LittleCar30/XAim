//! 基于 ripgrep 核心库（`grep-regex` + `grep-searcher`）的项目内容检索。
//!
//! 行为要点：
//! - 并行遍历项目目录（[`ignore::WalkParallel`]），遵守各级 `.gitignore`
//!   （不要求项目在 git 仓库内），并在其上叠加文件树同款的默认忽略表与
//!   敏感文件规则（复用 [`crate::index::scanner`]），保证检索范围与文件树
//!   一致（不含 `.git`/`node_modules`/`.env` 等）；
//! - 默认按字面量、大小写不敏感匹配；`regex = true` 时按正则匹配；
//!   中文等 UTF-8 内容原生支持，行内非法 UTF-8 字节以替换字符展示；
//! - 每条命中的 `column` 为该行首个匹配的字符列（1 起，按 Unicode 字符
//!   计数而非字节偏移，中文下语义正确）；`text` 为整行内容，行尾换行
//!   剥离、超长行截断至 [`MAX_TEXT_CHARS`]；
//! - 结果按文件聚合（同文件命中相邻、行号升序），受 `max_results` 总量
//!   与 `max_per_file` 单文件上限双重约束，达到任一上限即尽早终止遍历。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use grep_matcher::Matcher as _;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::types::TypesBuilder;
use ignore::{DirEntry, Error as IgnoreError, WalkBuilder, WalkParallel, WalkState};
use serde::{Deserialize, Serialize};

use crate::errors::{Result, XaimError};
use crate::index::scanner::{is_sensitive, rel_path, DEFAULT_IGNORES};

/// 单条命中 `text` 的最大保留字符数，超出部分截断，避免压缩后的超长行撑爆结果。
pub const MAX_TEXT_CHARS: usize = 1000;

/// 单条检索命中。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepMatch {
    /// 相对项目根、以 `/` 分隔的文件路径。
    pub path: String,
    /// 1 起的行号。
    pub line: u32,
    /// 该行首个匹配的列号：1 起，按 Unicode 字符计数（非字节偏移）。
    pub column: u32,
    /// 整行内容（不含行尾换行，超长行截断至 [`MAX_TEXT_CHARS`] 字符）。
    pub text: String,
}

/// 检索选项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GrepOptions {
    /// 结果总数上限，默认 200。
    pub max_results: usize,
    /// 单个文件的命中上限，默认 5。
    pub max_per_file: usize,
    /// 是否区分大小写，默认 false（不区分）。
    pub case_sensitive: bool,
    /// 是否按正则匹配，默认 false（按字面量匹配）。
    pub regex: bool,
    /// 限定文件类型（ripgrep 类型名，如 rust/python/ts/js/markdown），空表示不过滤。
    pub file_types: Vec<String>,
}

impl Default for GrepOptions {
    fn default() -> Self {
        Self {
            max_results: 200,
            max_per_file: 5,
            case_sensitive: false,
            regex: false,
            file_types: Vec::new(),
        }
    }
}

/// 在 `root` 下检索 `query`，返回按文件聚合（同文件相邻、行号升序）的命中。
pub fn search(root: &Path, query: &str, opts: GrepOptions) -> Result<Vec<GrepMatch>> {
    if !root.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("项目根目录不存在或不是目录: {}", root.display()),
        )
        .into());
    }
    // 空查询或零上限都没有可产出的结果，直接短路（也免去无谓的全量遍历）。
    if query.trim().is_empty() || opts.max_results == 0 || opts.max_per_file == 0 {
        return Ok(Vec::new());
    }

    let matcher = build_matcher(query, &opts)?;
    let walker = build_walker(root, &opts)?;

    let job = Arc::new(Job {
        root: root.to_path_buf(),
        matcher,
        matches: Mutex::new(Vec::new()),
        quit: AtomicBool::new(false),
        max_results: opts.max_results,
        max_per_file: opts.max_per_file,
    });

    let walker_job = Arc::clone(&job);
    walker.run(move || {
        let job = Arc::clone(&walker_job);
        // 每线程一个 Searcher：内部持有行缓冲，不可跨线程共享，但可复用。
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .build();
        Box::new(move |entry| visit_entry(&job, &mut searcher, entry))
    });

    let mut matches = {
        let mut collected = job
            .matches
            .lock()
            .map_err(|_| io::Error::other("检索结果累加器已中毒"))?;
        std::mem::take(&mut *collected)
    };

    // 并行遍历的产出顺序与线程调度相关，统一按 (路径, 行, 列) 排序，
    // 既保证确定性，也让同一文件的命中相邻（按文件聚合）。
    matches.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
    });
    Ok(matches)
}

/// 一次检索的共享状态，工作线程经 `Arc<Job>` 访问。
struct Job {
    root: PathBuf,
    matcher: RegexMatcher,
    /// 已收集的命中，长度恒不超过 `max_results`。
    matches: Mutex<Vec<GrepMatch>>,
    /// 总量达到上限后置位，通知遍历尽早退出。
    quit: AtomicBool,
    max_results: usize,
    max_per_file: usize,
}

/// 处理遍历到的单个条目：文件则交给 searcher 检索，其余跳过。
fn visit_entry(
    job: &Job,
    searcher: &mut Searcher,
    entry: std::result::Result<DirEntry, IgnoreError>,
) -> WalkState {
    let entry = match entry {
        Ok(entry) => entry,
        Err(err) => {
            tracing::debug!(%err, "检索跳过不可读条目");
            return WalkState::Continue;
        }
    };
    if job.quit.load(Ordering::Relaxed) {
        return WalkState::Quit;
    }
    if !entry.file_type().is_some_and(|ft| ft.is_file()) {
        return WalkState::Continue;
    }

    let path = entry.into_path();
    let rel = rel_path(&job.root, &path);
    let mut sink = MatchSink {
        rel_path: rel.as_str(),
        job,
        emitted: 0,
    };
    if let Err(err) = searcher.search_path(&job.matcher, &path, &mut sink) {
        tracing::debug!(path = %rel, %err, "检索文件失败，跳过");
    }
    WalkState::Continue
}

/// 单个文件的命中收集器：负责行文本组装与两级上限控制。
struct MatchSink<'a> {
    rel_path: &'a str,
    job: &'a Job,
    /// 本文件已收集的命中数。
    emitted: usize,
}

impl Sink for MatchSink<'_> {
    type Error = io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        mat: &SinkMatch<'_>,
    ) -> std::result::Result<bool, io::Error> {
        let mut matches = self
            .job
            .matches
            .lock()
            .map_err(|_| io::Error::other("检索结果累加器已中毒"))?;
        // 总量已满：置位退出标记，并停止当前文件。
        if matches.len() >= self.job.max_results {
            self.job.quit.store(true, Ordering::Relaxed);
            return Ok(false);
        }
        // 单文件已满：仅停止当前文件，遍历继续。
        if self.emitted >= self.job.max_per_file {
            return Ok(false);
        }
        self.emitted += 1;
        matches.push(build_match(self.rel_path, mat, &self.job.matcher));
        if matches.len() >= self.job.max_results {
            self.job.quit.store(true, Ordering::Relaxed);
        }
        Ok(true)
    }
}

/// 把 searcher 报告的命中行折叠成 [`GrepMatch`]。
fn build_match(path: &str, mat: &SinkMatch<'_>, matcher: &RegexMatcher) -> GrepMatch {
    let line = trim_terminator(mat.bytes());
    // Searcher 默认开启行号，line_number 恒为 Some，0 仅是防御性兜底。
    let line_no = mat.line_number().unwrap_or(0);
    // searcher 只上报命中行，find 必有结果；极端情况下列号退回 0。
    let column = match matcher.find(line) {
        Ok(Some(m)) => char_column(line, m.start()),
        _ => 0,
    };
    let mut text = String::from_utf8_lossy(line).into_owned();
    if let Some((cut, _)) = text.char_indices().nth(MAX_TEXT_CHARS) {
        text.truncate(cut);
    }
    GrepMatch {
        path: path.to_string(),
        line: u32::try_from(line_no).unwrap_or(u32::MAX),
        column,
        text,
    }
}

/// 构建底层正则：默认按字面量且大小写不敏感，`regex = true` 时按正则解析。
fn build_matcher(query: &str, opts: &GrepOptions) -> Result<RegexMatcher> {
    let mut builder = RegexMatcherBuilder::new();
    builder
        .fixed_strings(!opts.regex)
        .case_insensitive(!opts.case_sensitive);
    builder
        .build(query)
        .map_err(|err| XaimError::Config(format!("检索模式 `{query}` 非法: {err}")))
}

/// 构建并行遍历器：遵守 .gitignore，叠加文件树同款的默认忽略与敏感文件规则。
fn build_walker(root: &Path, opts: &GrepOptions) -> Result<WalkParallel> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false) // 与文件树一致：保留隐藏文件，.git 由默认忽略表过滤
        .require_git(false) // 非 git 仓库也遵守 .gitignore
        .git_global(false) // 不读用户全局 gitignore，结果只取决于项目自身
        .parents(false) // 只应用项目内的 ignore 规则，不外溢到父目录
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !(is_ignored_name(&name) || is_sensitive(&name))
        });

    if opts.file_types.is_empty() {
        return Ok(builder.build_parallel());
    }

    let mut types = TypesBuilder::new();
    types.add_defaults();
    for ty in &opts.file_types {
        types.select(ty);
    }
    let types = types
        .build()
        .map_err(|err| XaimError::Config(format!("文件类型过滤非法: {err}")))?;
    builder.types(types);
    Ok(builder.build_parallel())
}

/// 条目名是否命中默认忽略表（`.git`、`node_modules`、`target` 等）。
fn is_ignored_name(name: &str) -> bool {
    DEFAULT_IGNORES.contains(&name)
}

/// 去掉行尾的 `\n` / `\r\n`（grep-searcher 返回的行包含行结束符）。
fn trim_terminator(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &line[..end]
}

/// 匹配起始字节位转 1 起字符列；前缀非合法 UTF-8 时退回字节列。
fn char_column(line: &[u8], byte_pos: usize) -> u32 {
    let column = std::str::from_utf8(&line[..byte_pos])
        .map(|prefix| prefix.chars().count() + 1)
        .unwrap_or(byte_pos + 1);
    u32::try_from(column).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    use tempfile::tempdir;

    /// 在 `root` 下创建相对路径 `rel` 的文件（自动补建父目录）。
    fn write_file(root: &Path, rel: &str, content: impl AsRef<[u8]>) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// 以默认选项检索并解包。
    fn search_default(root: &Path, query: &str) -> Vec<GrepMatch> {
        search(root, query, GrepOptions::default()).unwrap()
    }

    /// 提取命中路径序列。
    fn paths(matches: &[GrepMatch]) -> Vec<&str> {
        matches.iter().map(|m| m.path.as_str()).collect()
    }

    /// 断言结果按文件聚合：路径不降序，同文件内行号不降序。
    fn assert_grouped(matches: &[GrepMatch]) {
        assert!(
            matches.windows(2).all(|w| w[0].path <= w[1].path),
            "同文件的命中应相邻（路径有序）: {:?}",
            paths(matches)
        );
        assert!(
            matches
                .windows(2)
                .all(|w| w[0].path != w[1].path || w[0].line <= w[1].line),
            "同文件内命中应按行号升序"
        );
    }

    #[test]
    fn finds_chinese_matches_with_char_columns() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(
            root,
            "src/notes.md",
            "// 你好世界，这是中文注释\n世界和平\n",
        );
        let matches = search_default(root, "世界");

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].path, "src/notes.md");
        assert_eq!(matches[0].line, 1);
        // 前缀 `// 你好` 共 5 个字符，字节列会算出 10，字符列应为 6。
        assert_eq!(matches[0].column, 6);
        assert_eq!(matches[0].text, "// 你好世界，这是中文注释");
        assert_eq!(matches[1].line, 2);
        assert_eq!(matches[1].column, 1);
    }

    #[test]
    fn supports_chinese_file_paths() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "文档/说明.md", "这里有关键词\n");

        let matches = search_default(root, "关键词");

        assert_eq!(paths(&matches), vec!["文档/说明.md"]);
        assert_eq!(matches[0].text, "这里有关键词");
    }

    #[test]
    fn case_insensitive_by_default_and_sensitive_on_demand() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "a.txt", b"Hello World\n");

        let matches = search_default(root, "hello");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].text, "Hello World");

        let opts = GrepOptions {
            case_sensitive: true,
            ..GrepOptions::default()
        };
        assert!(search(root, "hello", opts).unwrap().is_empty());
    }

    #[test]
    fn literal_mode_ignores_regex_metachars() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "lit.txt", b"call f.n now\n");
        write_file(root, "fun.txt", b"call fun now\n");

        let matches = search_default(root, "f.n");
        assert_eq!(paths(&matches), vec!["lit.txt"], "字面量模式下 . 不应通配");

        let opts = GrepOptions {
            regex: true,
            ..GrepOptions::default()
        };
        let matches = search(root, "f.n", opts).unwrap();
        assert_eq!(
            paths(&matches),
            vec!["fun.txt", "lit.txt"],
            "正则模式下 . 通配"
        );
    }

    #[test]
    fn invalid_regex_is_config_error_but_literal_is_fine() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "a.txt", b"hello (world)\n");

        let opts = GrepOptions {
            regex: true,
            ..GrepOptions::default()
        };
        let err = search(root, "([", opts).unwrap_err();
        assert!(
            matches!(err, XaimError::Config(_)),
            "非法正则应返回 Config 错误"
        );

        assert!(search_default(root, "([").is_empty());
    }

    #[test]
    fn respects_gitignore() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, ".gitignore", b"generated/\n");
        write_file(root, "generated/out.txt", b"needle\n");
        write_file(root, "src/in.txt", b"needle\n");

        assert_eq!(paths(&search_default(root, "needle")), vec!["src/in.txt"]);
    }

    #[test]
    fn skips_default_ignore_dirs_and_sensitive_files() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "node_modules/pkg.js", b"needle\n");
        write_file(root, "target/debug/app.rs", b"needle\n");
        write_file(root, ".git/HEAD", b"needle\n");
        write_file(root, ".env", b"SECRET=needle\n");
        write_file(root, "server.key", b"needle\n");
        write_file(root, "src/main.rs", b"needle\n");

        assert_eq!(paths(&search_default(root, "needle")), vec!["src/main.rs"]);
    }

    #[test]
    fn per_file_limit_stops_early_but_keeps_other_files() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "many.txt", b"needle\n".repeat(10).as_slice());
        write_file(root, "one.txt", b"needle\n");

        let opts = GrepOptions {
            max_per_file: 3,
            ..GrepOptions::default()
        };
        let matches = search(root, "needle", opts).unwrap();

        assert_eq!(matches.len(), 4, "many.txt 取前 3 条 + one.txt 1 条");
        assert_eq!(
            matches
                .iter()
                .filter(|m| m.path == "many.txt")
                .map(|m| m.line)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(paths(&matches).last(), Some(&"one.txt"));
    }

    #[test]
    fn total_limit_caps_results_exactly() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        for d in 0..6u32 {
            write_file(
                root,
                &format!("mod_{d:02}/file.txt"),
                b"needle\n".repeat(4).as_slice(),
            );
        }

        let opts = GrepOptions {
            max_results: 10,
            ..GrepOptions::default()
        };
        let matches = search(root, "needle", opts).unwrap();

        assert_eq!(matches.len(), 10, "结果应精确截断到 max_results");
        assert_grouped(&matches);
        // 并行遍历下哪个文件在何时被截断取决于线程调度：可能只有一个文件
        // 贡献 2 条，也可能多个文件各贡献 1~3 条。可靠契约是：每个文件至多
        // 4 条（max_per_file），总数恰为 10（max_results）。
        let per_file: Vec<usize> = matches
            .chunk_by(|a, b| a.path == b.path)
            .map(|group| group.len())
            .collect();
        assert!(per_file.iter().all(|&n| n <= 4), "{per_file:?} 每组应 ≤ 4");
    }

    #[test]
    fn groups_matches_by_file_sorted_by_line() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "b.txt", b"needle 1\nplain\nneedle 2\n");
        write_file(root, "a.txt", b"needle\n");
        write_file(root, "c/inner.txt", b"needle\n");

        let matches = search_default(root, "needle");
        assert_eq!(
            paths(&matches),
            vec!["a.txt", "b.txt", "b.txt", "c/inner.txt"]
        );
        assert_eq!(
            matches
                .iter()
                .filter(|m| m.path == "b.txt")
                .map(|m| m.line)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
    }

    #[test]
    fn file_types_filter_by_ripgrep_names() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "main.rs", b"needle\n");
        write_file(root, "main.py", b"needle\n");
        write_file(root, "main.md", b"needle\n");

        let opts = |types: &[&str]| GrepOptions {
            file_types: types.iter().map(|t| t.to_string()).collect(),
            ..GrepOptions::default()
        };

        let matches = search(root, "needle", opts(&["python"])).unwrap();
        assert_eq!(paths(&matches), vec!["main.py"]);

        let matches = search(root, "needle", opts(&["rust", "markdown"])).unwrap();
        assert_eq!(paths(&matches), vec!["main.md", "main.rs"]);

        let err = search(root, "needle", opts(&["nosuchtype"])).unwrap_err();
        assert!(
            matches!(err, XaimError::Config(_)),
            "未知类型名应返回 Config 错误"
        );
    }

    #[test]
    fn binary_files_are_skipped() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "blob.bin", b"\x00needle");
        write_file(root, "text.txt", b"needle\n");

        let matches = search_default(root, "needle");
        assert_eq!(paths(&matches), vec!["text.txt"]);
    }

    #[test]
    fn crlf_line_endings_are_trimmed() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "win.txt", "你好世界\r\nneedle\r\n");

        let matches = search_default(root, "你好世界");
        assert_eq!(matches[0].text, "你好世界", "CRLF 不应残留在行文本中");
    }

    #[test]
    fn overlong_lines_are_truncated() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let mut line = String::from("needle");
        line.push_str(&"x".repeat(2000));
        line.push('\n');
        write_file(root, "long.txt", line.as_bytes());

        let matches = search_default(root, "needle");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].text.chars().count(), MAX_TEXT_CHARS);
        assert!(matches[0].text.starts_with("needle"));
    }

    #[test]
    fn empty_query_returns_empty() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "a.txt", b"anything\n");

        assert!(search_default(root, "").is_empty());
        assert!(search_default(root, "   ").is_empty());
    }

    #[test]
    fn zero_limits_return_empty() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        write_file(root, "a.txt", b"needle\n");

        for max_results in [0, 200] {
            for max_per_file in [0, 5] {
                if max_results == 0 || max_per_file == 0 {
                    let opts = GrepOptions {
                        max_results,
                        max_per_file,
                        ..GrepOptions::default()
                    };
                    assert!(search(root, "needle", opts).unwrap().is_empty());
                }
            }
        }
    }

    #[test]
    fn missing_root_is_io_error() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let err = search(&missing, "needle", GrepOptions::default()).unwrap_err();
        assert!(matches!(err, XaimError::Io(_)));
    }

    #[test]
    fn medium_project_searches_within_300ms() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        for d in 0..40u32 {
            for f in 0..25u32 {
                let content: String = (0..12u32)
                    .map(|i| format!("第{i}行 你好世界 hello world\n"))
                    .collect();
                write_file(
                    root,
                    &format!("mod_{d:02}/file_{f:02}.txt"),
                    content.as_bytes(),
                );
            }
        }

        let opts = GrepOptions {
            max_results: 100_000,
            max_per_file: 1_000,
            ..GrepOptions::default()
        };
        let start = Instant::now();
        let matches = search(root, "你好世界", opts).unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(300),
            "中型项目（1000 文件 × 12 行）检索耗时 {elapsed:?}，应 < 300ms"
        );
        println!("中型项目（1000 文件 × 12 行，12000 命中）检索耗时 {elapsed:?}");
        assert_eq!(matches.len(), 40 * 25 * 12, "不应因上限提前截断");
        assert_grouped(&matches);
    }
}
