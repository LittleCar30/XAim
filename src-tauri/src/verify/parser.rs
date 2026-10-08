//! 验证输出解析：把各工具的文本输出转成结构化诊断，供修复回路喂给模型。
//!
//! 每种工具一个解析器，按行提取 `文件:行:列 + 消息 + 级别`；行内无法定位的
//! 错误（如 `error: could not compile`）保留为空定位。所有解析器只做匹配与
//! 提取，不匹配的行直接跳过，绝不 panic；结果按全字段去重并保留首次顺序。

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

/// 从验证输出解析出的一条诊断错误。
///
/// `line`/`column` 为工具原始的 1 基坐标，无法定位时为 0；`file` 为空表示
/// 该错误不指向具体文件。`severity` 取 `error`/`warning`。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiagError {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
    pub severity: String,
}

/// 构造一条诊断。
fn diag(file: &str, line: u32, column: u32, message: &str, severity: &str) -> DiagError {
    DiagError {
        file: file.to_string(),
        line,
        column,
        message: message.to_string(),
        severity: severity.to_string(),
    }
}

/// 按全字段去重，保留首次出现顺序。
fn dedup(errors: Vec<DiagError>) -> Vec<DiagError> {
    let mut seen = HashSet::new();
    errors
        .into_iter()
        .filter(|e| seen.insert(e.clone()))
        .collect()
}

/// 把 u32 文本解析为数字，非法或溢出时按 0（无定位）处理。
fn num(text: &str) -> u32 {
    text.parse().unwrap_or(0)
}

// tsc 紧凑格式：src/app.ts(10,5): error TS2339: ...
static TSC_PAREN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(.+?)\((\d+),(\d+)\):\s+(error|warning)\s+(.*)$"#).unwrap());
// tsc --pretty 格式：src/app.ts:10:5 - error TS2339: ...
static TSC_DASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?):(\d+):(\d+)\s+-\s+(error|warning)\s+(.*)$").unwrap());

/// 解析 tsc 输出（`file(line,col): error TSxxxx: msg` 与 `--pretty` 的
/// `file:line:col - error` 两种格式）。
pub fn parse_tsc(output: &str) -> Vec<DiagError> {
    let mut errors = Vec::new();
    for line in output.lines() {
        if let Some(c) = TSC_PAREN.captures(line).or_else(|| TSC_DASH.captures(line)) {
            errors.push(diag(&c[1], num(&c[2]), num(&c[3]), &c[5], &c[4]));
        }
    }
    dedup(errors)
}

// eslint stylish 条目：`  10:5   error    消息  规则名`（依赖缩进与文件行区分）。
static ESLINT_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+(\d+):(\d+)\s+(error|warning)\s+(.+?)\s*$").unwrap());
// 消息尾部的规则名（与消息之间至少两个空格）。
static ESLINT_RULE_TAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*\S)\s{2,}(\S+)$").unwrap());

/// 解析 eslint 默认 stylish 输出：无缩进行为文件名，缩进行为诊断条目，
/// `✖ N problems` 汇总行忽略。
pub fn parse_eslint(output: &str) -> Vec<DiagError> {
    let mut errors = Vec::new();
    let mut current_file = String::new();
    for raw in output.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if !raw.starts_with(' ') && !raw.starts_with('\t') {
            if !line.starts_with('✖') {
                current_file = line.trim().to_string();
            }
            continue;
        }
        let Some(c) = ESLINT_ITEM.captures(line) else {
            continue;
        };
        if current_file.is_empty() {
            continue;
        }
        // 规则名与消息之间隔两个以上空格；分离后以 `(规则名)` 附在消息尾部。
        let message = match ESLINT_RULE_TAIL.captures(&c[4]) {
            Some(rule) => format!("{} ({})", &rule[1], &rule[2]),
            None => c[4].to_string(),
        };
        errors.push(diag(&current_file, num(&c[1]), num(&c[2]), &message, &c[3]));
    }
    dedup(errors)
}

// pytest 汇总：FAILED tests/test_app.py::test_add - assert 2 == 3
static PYTEST_SUMMARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(FAILED|ERROR)\s+(.+)$").unwrap());
// pytest 回溯帧：tests/test_app.py:10: in test_add（--tb=line 则为错误文本）。
static PYTEST_FRAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\S+\.py):(\d+):\s*(.*)$").unwrap());
// Python 原生回溯：File "tests/app.py", line 10, in <module>
static PY_FILE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*File "(.+?)", line (\d+)"#).unwrap());

/// 解析 pytest 输出：`FAILED/ERROR` 汇总行、回溯帧与 Python 原生 traceback。
pub fn parse_pytest(output: &str) -> Vec<DiagError> {
    let mut errors = Vec::new();
    for raw in output.lines() {
        let line = raw.trim();
        if let Some(c) = PYTEST_SUMMARY.captures(line) {
            let body = &c[2];
            let (loc, detail) = match body.split_once(" - ") {
                Some((loc, msg)) => (loc, Some(msg)),
                None => (body, None),
            };
            let name = loc.rsplit("::").next().unwrap_or(loc);
            let message = match detail {
                Some(msg) => format!("{name}: {msg}"),
                None => name.to_string(),
            };
            let file = loc.split("::").next().unwrap_or(loc);
            errors.push(diag(file, 0, 0, &message, "error"));
        } else if let Some(c) = PYTEST_FRAME.captures(line) {
            let message = if c[3].trim().is_empty() {
                line
            } else {
                c[3].trim()
            };
            errors.push(diag(&c[1], num(&c[2]), 0, message, "error"));
        } else if let Some(c) = PY_FILE_LINE.captures(line) {
            errors.push(diag(&c[1], num(&c[2]), 0, line, "error"));
        }
    }
    dedup(errors)
}

// rustc 诊断主行：error[E0308]: mismatched types / warning: unused ...
static CARGO_PRIMARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(error|warning)(\[E\d+\])?:\s+(.*)$").unwrap());
// 诊断定位行： --> src/main.rs:10:24
static CARGO_LOCATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*-->\s+(\S+)\s*$").unwrap());
// cargo test 失败时的 panic 行：thread '...' panicked at src/lib.rs:10:5:
static CARGO_PANIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"panicked at (\S+):(\d+):(\d+):").unwrap());

/// 把 `file:line[:col]` 形态的文本从右往左拆成 (file, line, col)。
/// 用 rsplit 而非贪婪正则：路径里可能含冒号（Windows 盘符、目录名带数字），
/// 从右端取数字段是唯一无歧义的拆法；无法解析时返回 `None`。
fn split_location(text: &str) -> Option<(&str, u32, u32)> {
    let mut parts = text.rsplitn(3, ':');
    let third = parts.next()?;
    let second = parts.next()?;
    match parts.next() {
        Some(file) => Some((file, second.parse().ok()?, third.parse().ok()?)),
        None => Some((second, third.parse().ok()?, 0)),
    }
}

/// 解析 cargo/rustc 输出：诊断主行 + `-->` 定位行配对；主行缺定位时保留
/// 空定位（如 `error: could not compile`）；`cargo test` 的 panic 行按
/// `panicked at file:line:col` 直接定位，并附带下一行的失败载荷。
pub fn parse_cargo(output: &str) -> Vec<DiagError> {
    let mut errors = Vec::new();
    // 等待配对定位行的诊断主行：(severity, message)。
    let mut pending: Option<(String, String)> = None;
    let flush = |pending: &mut Option<(String, String)>,
                 errors: &mut Vec<DiagError>,
                 file: &str,
                 line: u32,
                 column: u32| {
        if let Some((severity, message)) = pending.take() {
            errors.push(diag(file, line, column, &message, &severity));
        }
    };

    let mut lines = output.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(c) = CARGO_PRIMARY.captures(line) {
            flush(&mut pending, &mut errors, "", 0, 0);
            let message = match c.get(2).map(|m| m.as_str()) {
                Some(code) => format!("{}: {}", code.trim_matches(['[', ']']), &c[3]),
                None => c[3].to_string(),
            };
            pending = Some((c[1].to_string(), message));
        } else if let Some(c) = CARGO_LOCATION.captures(line) {
            let (file, line_no, column) = split_location(&c[1]).unwrap_or(("", 0, 0));
            flush(&mut pending, &mut errors, file, line_no, column);
        } else if let Some(c) = CARGO_PANIC.captures(line) {
            let mut message = line.trim().to_string();
            // panic 载荷在下一行（如 `assertion \`left == right\` failed`）。
            if let Some(next) = lines.peek() {
                let next = next.trim();
                if !next.is_empty()
                    && !next.starts_with("note:")
                    && CARGO_PRIMARY.captures(next).is_none()
                    && CARGO_LOCATION.captures(next).is_none()
                {
                    message.push('\n');
                    message.push_str(next);
                    lines.next();
                }
            }
            errors.push(diag(&c[1], num(&c[2]), num(&c[3]), &message, "error"));
        }
    }
    flush(&mut pending, &mut errors, "", 0, 0);
    dedup(errors)
}

// go 工具定位行：./util.go:12:5: undeclared name: bar（列可缺省）。
static GO_LOCATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?\.go):(\d+):(?:(\d+):)?\s*(.*)$").unwrap());

/// 解析 go vet/build 输出（`file:line:col: msg`）与 go test 输出
/// （`--- FAIL: TestX` 与 `file:line: msg` 日志行）。
pub fn parse_go(output: &str) -> Vec<DiagError> {
    let mut errors = Vec::new();
    for raw in output.lines() {
        let line = raw.trim();
        if line.starts_with("--- FAIL:") {
            errors.push(diag("", 0, 0, line, "error"));
            continue;
        }
        if let Some(c) = GO_LOCATION.captures(line) {
            if !c[4].trim().is_empty() {
                errors.push(diag(
                    &c[1],
                    num(&c[2]),
                    num(c.get(3).map_or("", |m| m.as_str())),
                    &c[4],
                    "error",
                ));
            }
        }
    }
    dedup(errors)
}

// 通用编译器定位：file.ext:line[:col]: [error|warning|note:] 消息。
static GENERIC_LOCATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(.+?\.\w{1,10}):(\d+):(?:(\d+):)?\s*(?:(error|warning|fatal error|note):\s*)?(\S.*)$",
    )
    .unwrap()
});
// maven/javac 定位：[ERROR] /path/File.java:[10,5] cannot find symbol
static GENERIC_MAVEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?\.\w{1,10}):\[(\d+),(\d+)\]\s*(\S.*)$").unwrap());

/// 通用兜底解析：提取常见 `file:line:col:` 形态（javac/gcc/clang 等），
/// 兼容 maven 的 `[ERROR]`/`[WARNING]` 前缀与 `File.java:[10,5]` 定位；
/// `note:` 行是上下文噪音，跳过。
pub fn parse_generic(output: &str) -> Vec<DiagError> {
    let mut errors = Vec::new();
    for raw in output.lines() {
        let mut line = raw.trim();
        let mut severity = "error";
        if let Some(rest) = line.strip_prefix("[WARNING] ") {
            severity = "warning";
            line = rest;
        } else if let Some(rest) = line.strip_prefix("[ERROR] ") {
            line = rest;
        }
        if let Some(c) = GENERIC_MAVEN.captures(line) {
            errors.push(diag(&c[1], num(&c[2]), num(&c[3]), &c[4], severity));
            continue;
        }
        let Some(c) = GENERIC_LOCATION.captures(line) else {
            continue;
        };
        let line_severity = c.get(4).map(|m| m.as_str());
        if line_severity == Some("note") {
            continue;
        }
        let severity = match line_severity {
            Some("warning") => "warning",
            _ => severity,
        };
        errors.push(diag(
            &c[1],
            num(&c[2]),
            num(c.get(3).map_or("", |m| m.as_str())),
            &c[5],
            severity,
        ));
    }
    dedup(errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_overflow_is_safe_and_crlf_paths_keep_drive_prefix() {
        let output = "C:\\work\\app.ts(4294967296,3): warning TS1: overflow\r\nC:\\work\\app.ts(2,4): error TS2: bad type\r\n";
        assert_eq!(
            parse_tsc(output),
            vec![
                diag(r"C:\work\app.ts", 0, 3, "TS1: overflow", "warning"),
                diag(r"C:\work\app.ts", 2, 4, "TS2: bad type", "error"),
            ]
        );
    }

    #[test]
    fn eslint_orphan_entries_are_skipped_and_file_context_changes() {
        let output = "  1:2 error orphan\nfirst.js\n  2:3 warning plain message\n\nsecond.js\n  4:5 error other message\n";
        assert_eq!(
            parse_eslint(output),
            vec![
                diag("first.js", 2, 3, "plain message", "warning"),
                diag("second.js", 4, 5, "other message", "error"),
            ]
        );
    }

    #[test]
    fn cargo_pending_diagnostics_are_flushed_without_stealing_next_error() {
        let output = "warning: no location\nerror[E0001]: located\n --> src/lib.rs:2:3\nthread 'test' panicked at src/lib.rs:4:5:\nnote: backtrace disabled\nerror: final failure\n";
        let diagnostics = parse_cargo(output);
        assert_eq!(diagnostics.len(), 4);
        assert_eq!(diagnostics[0], diag("", 0, 0, "no location", "warning"));
        assert_eq!(
            diagnostics[1],
            diag("src/lib.rs", 2, 3, "E0001: located", "error")
        );
        assert!(!diagnostics[2].message.contains("note:"));
        assert_eq!(diagnostics[3], diag("", 0, 0, "final failure", "error"));
    }

    #[test]
    fn dedup_preserves_different_severities_and_first_seen_order() {
        let output = "a.c:2:3: warning: message\na.c:2:3: error: message\na.c:2:3: warning: message\nb.c:1: error: another\n";
        assert_eq!(
            parse_generic(output),
            vec![
                diag("a.c", 2, 3, "message", "warning"),
                diag("a.c", 2, 3, "message", "error"),
                diag("b.c", 1, 0, "another", "error"),
            ]
        );
    }

    #[test]
    fn parse_tsc_parses_paren_format() {
        let output =
            "src/app.ts(10,5): error TS2339: Property 'foo' does not exist on type 'Bar'.\n\
                      另一段无关日志\n\
                      src/index.tsx(3,12): error TS2304: Cannot find name 'baz'.\n\
                      2 errors found\n";
        let errors = parse_tsc(output);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].file, "src/app.ts");
        assert_eq!(errors[0].line, 10);
        assert_eq!(errors[0].column, 5);
        assert_eq!(errors[0].severity, "error");
        assert_eq!(
            errors[0].message,
            "TS2339: Property 'foo' does not exist on type 'Bar'."
        );
        assert_eq!(errors[1].file, "src/index.tsx");
        assert_eq!(errors[1].line, 3);
        assert_eq!(errors[1].column, 12);
    }

    #[test]
    fn parse_tsc_parses_pretty_format() {
        let output =
            "src/app.ts:10:5 - error TS2339: Property 'foo' does not exist on type 'Bar'.\n";
        let errors = parse_tsc(output);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "src/app.ts");
        assert_eq!(errors[0].line, 10);
        assert_eq!(errors[0].column, 5);
        assert_eq!(errors[0].severity, "error");
        assert_eq!(
            errors[0].message,
            "TS2339: Property 'foo' does not exist on type 'Bar'."
        );
    }

    #[test]
    fn parse_eslint_parses_stylish_output() {
        let output = "src\\app.ts\n\
                      \x20 10:5   error    Unexpected var, use let or const instead  no-var\n\
                      \x20 12:1   warning  'foo' is defined but never used  no-unused-vars\n\
                      \n\
                      src\\broken.js\n\
                      \x20 1:1  error  Parsing error: Unexpected token\n\
                      \n\
                      ✖ 3 problems (2 errors, 1 warning)\n";
        let errors = parse_eslint(output);
        assert_eq!(errors.len(), 3);
        assert_eq!(errors[0].file, "src\\app.ts");
        assert_eq!(errors[0].line, 10);
        assert_eq!(errors[0].column, 5);
        assert_eq!(errors[0].severity, "error");
        assert_eq!(
            errors[0].message,
            "Unexpected var, use let or const instead (no-var)"
        );
        assert_eq!(errors[1].severity, "warning");
        assert_eq!(
            errors[1].message,
            "'foo' is defined but never used (no-unused-vars)"
        );
        // 文件名应作用于其后的所有条目。
        assert_eq!(errors[2].file, "src\\broken.js");
        assert_eq!(errors[2].message, "Parsing error: Unexpected token");
    }

    #[test]
    fn parse_pytest_parses_summary_and_frames() {
        let output = "============================= test session starts =============================\n\
                      tests/test_app.py F.E                                                    [100%]\n\
                      =================================== FAILURES ===================================\n\
                          tests/test_app.py:10: in test_add\n\
                              assert add(1, 1) == 3\n\
                      E       assert 2 == 3\n\
                      =========================== short test summary info ============================\n\
                      FAILED tests/test_app.py::test_add - assert 2 == 3\n\
                      ERROR tests/test_loader.py::test_load - ImportError: cannot import name 'x'\n";
        let errors = parse_pytest(output);
        assert_eq!(errors.len(), 3);

        // 回溯帧给出准确行号（帧行在输出中先于汇总行出现）。
        let frame = errors.iter().find(|e| e.line == 10).expect("应包含回溯帧");
        assert_eq!(frame.file, "tests/test_app.py");
        assert_eq!(frame.message, "in test_add");

        // 汇总行：文件取 `::` 前的路径，消息带测试名与断言文本，行号未知为 0。
        let failed = errors
            .iter()
            .find(|e| e.line == 0 && e.file == "tests/test_app.py")
            .expect("应包含 FAILED 汇总");
        assert_eq!(failed.message, "test_add: assert 2 == 3");
        let errored = errors
            .iter()
            .find(|e| e.file == "tests/test_loader.py")
            .expect("应包含 ERROR 汇总");
        assert_eq!(
            errored.message,
            "test_load: ImportError: cannot import name 'x'"
        );
    }

    #[test]
    fn parse_pytest_parses_python_traceback_files() {
        let output = "Traceback (most recent call last):\n\
                      \x20 File \"/opt/app/loader.py\", line 8, in <module>\n\
                      \x20   import config\n\
                      ImportError: cannot import name 'x'\n";
        let errors = parse_pytest(output);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "/opt/app/loader.py");
        assert_eq!(errors[0].line, 8);
        assert_eq!(errors[0].severity, "error");
    }

    #[test]
    fn parse_cargo_parses_rustc_diagnostics() {
        let output = "   Compiling xaim v0.1.0 (D:\\code\\project\\XAim\\src-tauri)\n\
                      error[E0308]: mismatched types\n\
                      \x20--> src/main.rs:10:24\n\
                      \x20 |\n\
                      10 |     let x: u32 = \"s\";\n\
                      \x20  |            ^^^ expected `u32`, found `&str`\n\
                      \n\
                      warning: unused variable: `y`\n\
                      \x20--> src/lib.rs:2:9\n\
                      \n\
                      error: could not compile `xaim` (lib) due to 1 previous error\n";
        let errors = parse_cargo(output);
        assert_eq!(errors.len(), 3);

        assert_eq!(errors[0].file, "src/main.rs");
        assert_eq!(errors[0].line, 10);
        assert_eq!(errors[0].column, 24);
        assert_eq!(errors[0].severity, "error");
        assert_eq!(errors[0].message, "E0308: mismatched types");

        assert_eq!(errors[1].file, "src/lib.rs");
        assert_eq!(errors[1].line, 2);
        assert_eq!(errors[1].column, 9);
        assert_eq!(errors[1].severity, "warning");
        assert_eq!(errors[1].message, "unused variable: `y`");

        // 无定位的诊断保留原文，定位字段为空。
        assert_eq!(errors[2].file, "");
        assert_eq!(errors[2].line, 0);
        assert_eq!(
            errors[2].message,
            "could not compile `xaim` (lib) due to 1 previous error"
        );
    }

    #[test]
    fn parse_cargo_parses_test_panic() {
        let output = "---- tests::addition stdout ----\n\
                      \n\
                      thread 'tests::addition' panicked at src/lib.rs:10:5:\n\
                      assertion `left == right` failed\n\
                      note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\
                      test result: FAILED. 0 passed; 1 failed; 0 ignored\n";
        let errors = parse_cargo(output);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].file, "src/lib.rs");
        assert_eq!(errors[0].line, 10);
        assert_eq!(errors[0].column, 5);
        assert_eq!(
            errors[0].message,
            "thread 'tests::addition' panicked at src/lib.rs:10:5:\nassertion `left == right` failed"
        );
    }

    #[test]
    fn parse_go_parses_vet_and_test_output() {
        let vet = parse_go(
            "# xaim/internal/util\n\
                            ./util.go:12:5: undeclared name: bar\n\
                            ./main.go:10:2: undefined: foo\n\
                            old.go:3: syntax error: unexpected eof, expected ';'\n",
        );
        assert_eq!(vet.len(), 3);
        assert_eq!(vet[0].file, "./util.go");
        assert_eq!(vet[0].line, 12);
        assert_eq!(vet[0].column, 5);
        assert_eq!(vet[0].message, "undeclared name: bar");
        assert_eq!(vet[2].file, "old.go");
        assert_eq!(vet[2].line, 3);
        assert_eq!(vet[2].column, 0, "缺省列应为 0");
        assert_eq!(vet[2].message, "syntax error: unexpected eof, expected ';'");

        let test = parse_go(
            "--- FAIL: TestAdd (0.00s)\n\
                             \x20   main_test.go:12: got 2, want 3\n\
                             FAIL\n",
        );
        assert_eq!(test.len(), 2);
        assert_eq!(test[0].file, "");
        assert_eq!(test[0].message, "--- FAIL: TestAdd (0.00s)");
        assert_eq!(test[1].file, "main_test.go");
        assert_eq!(test[1].line, 12);
        assert_eq!(test[1].message, "got 2, want 3");
    }

    #[test]
    fn parse_generic_parses_common_formats() {
        let output = "src/App.java:10: error: ';' expected\n\
                      main.c:4:9: error: use of undeclared identifier 'x'\n\
                      [ERROR] /path/to/Other.java:[10,5] cannot find symbol\n\
                      src/legacy.ts:2:1: warning: 'x' is deprecated\n\
                      main.c:8:1: note: previous definition of 'x' was here\n";
        let errors = parse_generic(output);
        assert_eq!(errors.len(), 4);

        assert_eq!(errors[0].file, "src/App.java");
        assert_eq!(errors[0].line, 10);
        assert_eq!(errors[0].column, 0);
        assert_eq!(errors[0].message, "';' expected");

        assert_eq!(errors[1].file, "main.c");
        assert_eq!(errors[1].line, 4);
        assert_eq!(errors[1].column, 9);

        // maven 定位格式 [行,列]。
        assert_eq!(errors[2].file, "/path/to/Other.java");
        assert_eq!(errors[2].line, 10);
        assert_eq!(errors[2].column, 5);
        assert_eq!(errors[2].message, "cannot find symbol");

        assert_eq!(errors[3].severity, "warning");
        assert_eq!(errors[3].message, "'x' is deprecated");
    }

    #[test]
    fn identical_diagnostics_are_deduplicated() {
        let output = "src/app.ts(10,5): error TS2339: Property 'foo' does not exist.\n\
                      src/app.ts(10,5): error TS2339: Property 'foo' does not exist.\n";
        let errors = parse_tsc(output);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn garbage_and_empty_input_are_safe() {
        let garbage = "\u{0}\u{1}随机噪音\n\x07\n:::::\n((()))\n";
        for parser in [
            parse_tsc as fn(&str) -> Vec<DiagError>,
            parse_eslint,
            parse_pytest,
            parse_cargo,
            parse_go,
            parse_generic,
        ] {
            assert_eq!(parser(""), Vec::<DiagError>::new());
            assert_eq!(parser(garbage), Vec::<DiagError>::new());
            assert_eq!(parser("error\n -->\n"), Vec::<DiagError>::new());
        }
    }

    #[test]
    fn split_location_takes_rightmost_numeric_fields() {
        assert_eq!(
            split_location("src/main.rs:10:24"),
            Some(("src/main.rs", 10, 24))
        );
        assert_eq!(
            split_location("src/main.rs:10"),
            Some(("src/main.rs", 10, 0))
        );
        // Windows 盘符与目录名里的数字不影响从右往左的拆分。
        assert_eq!(
            split_location("D:\\code\\v2\\main.rs:10:24"),
            Some(("D:\\code\\v2\\main.rs", 10, 24))
        );
        assert_eq!(split_location("no-coords"), None);
    }
}
