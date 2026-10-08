//! 验证执行器：在项目目录运行构建/测试/检查命令并产出结构化结果。
//!
//! 与 [`crate::tools::exec`] 同一执行纪律：命令串经引号感知分词后逐参数
//! spawn（不经 shell），拒绝 shell 元字符，stdin 置空防挂起，超过时限杀死
//! 进程（`kill_on_drop` 兜底）。Windows 上 pnpm/mvn/npx 等是 `.cmd`/`.bat`
//! 垫片，直接 spawn 会 NotFound，故先按 PATH + PATHEXT 解析成真实可执行文件。
//!
//! 结果语义：进程产出（退出码非零、超时）都是 `Ok(VerifyResult)`，
//! 供调用方统一记录；程序缺失等无法执行的故障才是 `Err`。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::errors::{Result, XaimError};
use crate::tools::exec::{split_command, truncate_middle, SHELL_METACHARS};

/// 未指定时限（传入 0）时使用的默认超时秒数。
pub const DEFAULT_TIMEOUT_SEC: u64 = 300;

/// 验证输出的总字符上限，超出保留头尾（报错信息多在尾部）。
const MAX_OUTPUT_CHARS: usize = 20_000;

/// 验证类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyKind {
    TypeCheck,
    Lint,
    Test,
    Build,
}

impl VerifyKind {
    /// 结果与日志中使用的稳定标识。
    pub fn as_str(self) -> &'static str {
        match self {
            VerifyKind::TypeCheck => "type_check",
            VerifyKind::Lint => "lint",
            VerifyKind::Test => "test",
            VerifyKind::Build => "build",
        }
    }
}

/// 单次验证的结构化结果。
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyResult {
    /// 验证类型标识（[`VerifyKind::as_str`]）。
    pub kind: String,
    /// 退出码 0 视为通过；非零退出与超时均为未通过。
    pub passed: bool,
    /// 合并后的 stdout/stderr 文本（超长截断），超时或无输出时为说明文字。
    pub output: String,
    /// 实际耗时（毫秒）。
    pub duration_ms: i64,
}

/// 解析有效时限：0 视为未指定，按 [`DEFAULT_TIMEOUT_SEC`] 处理。
fn effective_timeout(timeout_sec: u64) -> u64 {
    if timeout_sec == 0 {
        DEFAULT_TIMEOUT_SEC
    } else {
        timeout_sec
    }
}

/// 把程序名解析为可执行文件路径。
///
/// Windows 上按 PATH 逐目录尝试 PATHEXT 扩展（.com/.exe/.bat/.cmd），
/// 命中 `.cmd`/`.bat` 垫片才能被直接 spawn；未命中时原样返回，由系统
/// 按 PATH 兜底。非 Windows 交由系统解析即可。
#[cfg(windows)]
fn resolve_program(program: &str) -> PathBuf {
    if program.contains('\\') || program.contains('/') || Path::new(program).extension().is_some() {
        return PathBuf::from(program);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for ext in [".com", ".exe", ".bat", ".cmd"] {
                let candidate = dir.join(format!("{program}{ext}"));
                if candidate.is_file() {
                    return candidate;
                }
            }
            let bare = dir.join(program);
            if bare.is_file() {
                return bare;
            }
        }
    }
    PathBuf::from(program)
}

#[cfg(not(windows))]
fn resolve_program(program: &str) -> PathBuf {
    PathBuf::from(program)
}

/// 在 `root` 目录运行验证命令并产出结构化结果。
///
/// - 命令串分词后逐参数 spawn（不经 shell），stdin 置空防挂起；
/// - `timeout_sec` 为 0 时按 [`DEFAULT_TIMEOUT_SEC`]（300 秒）处理，
///   超时杀死进程并回收，卡住前已产出的输出仍会附带在结果里；
/// - 退出码 0 → `passed = true`，否则 `passed = false` 并回传输出；
/// - 程序不存在、`root` 无效等 spawn 失败 → `Err`。
pub async fn run_verify(
    kind: VerifyKind,
    cmd: &str,
    root: &Path,
    timeout_sec: u64,
) -> Result<VerifyResult> {
    let kind_label = kind.as_str();
    if cmd.trim().is_empty() {
        return Err(XaimError::Verify("验证命令为空".into()));
    }
    if let Some(ch) = cmd.chars().find(|c| SHELL_METACHARS.contains(c)) {
        return Err(XaimError::Verify(format!(
            "验证命令包含 shell 元字符 `{ch}`，不经 shell 执行，请改为单条命令"
        )));
    }
    let tokens = split_command(cmd)?;
    if tokens.is_empty() {
        return Err(XaimError::Verify("验证命令为空".into()));
    }
    let timeout = effective_timeout(timeout_sec);
    let program = resolve_program(&tokens[0]);

    tracing::debug!(
        kind = kind_label,
        command = cmd,
        cwd = %root.display(),
        timeout_sec = timeout,
        "开始执行验证命令"
    );
    let start = Instant::now();

    let spawned = Command::new(&program)
        .args(&tokens[1..])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!(kind = kind_label, command = cmd, "验证程序不存在");
            return Err(XaimError::Verify(format!(
                "验证程序 `{}` 不存在或不可执行（环境未安装？）",
                tokens[0]
            )));
        }
        Err(e) => return Err(e.into()),
    };

    // 先取走管道交给独立读取任务：缓冲归任务所有，超时杀死进程后
    // 已产出的输出仍可回收。管道由上方 Stdio::piped() 保证存在。
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| XaimError::Verify("验证命令 stdout 管道缺失".into()))?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| XaimError::Verify("验证命令 stderr 管道缺失".into()))?;
    let read_stdout = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf).await;
        buf
    });
    let read_stderr = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf).await;
        buf
    });

    let waited = tokio::time::timeout(Duration::from_secs(timeout), child.wait()).await;
    let duration_ms = start.elapsed().as_millis() as i64;
    match waited {
        Err(_elapsed) => {
            // 超时：主动杀死并回收，避免残留进程。
            let _ = child.kill().await;
            let _ = child.wait().await;
            let stdout = String::from_utf8_lossy(&read_stdout.await.unwrap_or_default())
                .trim_end()
                .to_string();
            let stderr = String::from_utf8_lossy(&read_stderr.await.unwrap_or_default())
                .trim_end()
                .to_string();
            let mut output = combine_output(&stdout, &stderr);
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&format!("验证命令超时（{timeout} 秒），进程已终止: {cmd}"));
            tracing::warn!(
                kind = kind_label,
                command = cmd,
                duration_ms,
                timeout_sec = timeout,
                "验证命令超时被终止"
            );
            Ok(VerifyResult {
                kind: kind_label.to_string(),
                passed: false,
                output,
                duration_ms,
            })
        }
        Ok(Err(e)) => Err(e.into()),
        Ok(Ok(status)) => {
            let stdout_bytes = read_stdout.await.unwrap_or_default();
            let stderr_bytes = read_stderr.await.unwrap_or_default();
            let stdout = String::from_utf8_lossy(&stdout_bytes);
            let stderr = String::from_utf8_lossy(&stderr_bytes);
            let passed = status.success();
            let text = combine_output(&stdout, &stderr);
            tracing::info!(
                kind = kind_label,
                command = cmd,
                passed,
                duration_ms,
                "验证命令执行完成"
            );
            Ok(VerifyResult {
                kind: kind_label.to_string(),
                passed,
                output: text,
                duration_ms,
            })
        }
    }
}

/// 合并 stdout 与 stderr：stdout 在前、stderr 标注在后，超长截断保留头尾；
/// 两者皆空时给出说明文字。
fn combine_output(stdout: &str, stderr: &str) -> String {
    let mut text = truncate_middle(stdout.trim_end(), MAX_OUTPUT_CHARS);
    let stderr = stderr.trim_end();
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("[stderr] ");
        text.push_str(&truncate_middle(stderr, MAX_OUTPUT_CHARS));
    }
    if text.is_empty() {
        text = "命令执行完成（无输出）".into();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_labels_are_stable() {
        assert_eq!(VerifyKind::TypeCheck.as_str(), "type_check");
        assert_eq!(VerifyKind::Lint.as_str(), "lint");
        assert_eq!(VerifyKind::Test.as_str(), "test");
        assert_eq!(VerifyKind::Build.as_str(), "build");
    }

    #[test]
    fn zero_timeout_falls_back_to_default() {
        assert_eq!(DEFAULT_TIMEOUT_SEC, 300);
        assert_eq!(effective_timeout(0), 300);
        assert_eq!(effective_timeout(7), 7);
    }

    #[test]
    fn combine_output_merges_streams_and_keeps_placeholder() {
        assert_eq!(combine_output("out", ""), "out");
        assert_eq!(combine_output("", "err"), "[stderr] err");
        assert_eq!(combine_output("out", "err"), "out\n[stderr] err");
        assert_eq!(combine_output("", ""), "命令执行完成（无输出）");
    }

    #[tokio::test]
    async fn runs_command_and_captures_output() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_verify(VerifyKind::Test, "hostname", dir.path(), 30)
            .await
            .unwrap();
        assert!(result.passed, "{}", result.output);
        assert_eq!(result.kind, "test");
        assert!(!result.output.is_empty(), "应捕获到输出");
        assert!(result.duration_ms >= 0);
    }

    #[tokio::test]
    async fn nonzero_exit_reports_failure_with_output() {
        let dir = tempfile::tempdir().unwrap();
        let command = if cfg!(windows) {
            "where xaim-no-such-file-xyz"
        } else {
            "ls /xaim-no-such-dir-xyz"
        };
        let result = run_verify(VerifyKind::Build, command, dir.path(), 30)
            .await
            .unwrap();
        assert!(!result.passed, "非零退出应视为未通过");
        assert_eq!(result.kind, "build");
        assert!(!result.output.is_empty(), "失败输出应回传");
    }

    #[tokio::test]
    async fn stderr_is_captured() {
        let dir = tempfile::tempdir().unwrap();
        // git 对未知子命令的报错走 stderr。
        let result = run_verify(
            VerifyKind::Lint,
            "git xaim-no-such-subcommand",
            dir.path(),
            30,
        )
        .await
        .unwrap();
        assert!(!result.passed);
        assert!(
            result.output.contains("[stderr]"),
            "stderr 应被捕获并标注: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn timeout_kills_long_running_command() {
        let dir = tempfile::tempdir().unwrap();
        let command = if cfg!(windows) {
            "ping -n 30 127.0.0.1"
        } else {
            "ping -c 30 127.0.0.1"
        };
        let start = std::time::Instant::now();
        let result = run_verify(VerifyKind::Test, command, dir.path(), 1)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert!(!result.passed);
        assert!(result.output.contains("超时"), "{}", result.output);
        assert!(
            result.duration_ms >= 1000 && result.duration_ms < 15_000,
            "duration_ms 应接近时限: {}",
            result.duration_ms
        );
        assert!(
            elapsed < Duration::from_secs(15),
            "超时应尽快终止进程，实际耗时 {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn missing_program_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_verify(
            VerifyKind::Lint,
            "xaim-fake-program-xyz --version",
            dir.path(),
            10,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, XaimError::Verify(_)));
        assert!(err.to_string().contains("不存在"));
    }

    #[tokio::test]
    async fn empty_and_metachar_commands_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "   ", "cargo build | head", "make a; make b"] {
            let err = run_verify(VerifyKind::Build, bad, dir.path(), 10)
                .await
                .unwrap_err();
            assert!(matches!(err, XaimError::Verify(_)), "{bad} 应被拒绝");
        }
    }

    #[tokio::test]
    async fn invalid_root_is_reported_not_panic() {
        let missing = std::env::temp_dir().join("xaim-missing-root-xyz");
        let err = run_verify(VerifyKind::Test, "hostname", &missing, 10)
            .await
            .unwrap_err();
        // current_dir 不存在时 spawn 失败，应以 Err 收场而非 panic。
        assert!(
            matches!(err, XaimError::Verify(_) | XaimError::Io(_)),
            "{err:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_resolves_program_from_path() {
        assert!(
            resolve_program("cmd").extension().is_some(),
            "cmd 应解析为带扩展名的绝对路径"
        );
        assert_eq!(
            resolve_program("xaim-fake-program-xyz"),
            PathBuf::from("xaim-fake-program-xyz"),
            "未命中应原样返回交由系统兜底"
        );
    }
}
