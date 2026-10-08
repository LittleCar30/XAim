//! Headless evaluations use the same orchestrator, providers and patch engine as XAim.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use xaim_lib::agent::orchestrator::{Orchestrator, OrchestratorConfig};
use xaim_lib::errors::{Result as XaimResult, XaimError};
use xaim_lib::events::XaimEvent;
use xaim_lib::model::provider::{CompletionRequest, CompletionResponse, ModelProvider};
use xaim_lib::repair::r#loop::EventSink;
use xaim_lib::{model, store, types};

#[cfg(windows)]
#[path = "process_job.rs"]
mod process_job;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalMode {
    Agent,
    Reference,
    Baseline,
}

impl std::fmt::Display for EvalMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Agent => "agent",
            Self::Reference => "reference",
            Self::Baseline => "baseline",
        })
    }
}

impl std::str::FromStr for EvalMode {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, String> {
        match value {
            "agent" => Ok(Self::Agent),
            "reference" => Ok(Self::Reference),
            "baseline" => Ok(Self::Baseline),
            _ => Err(format!("Unknown evaluation mode: {value}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EvalOptions {
    pub mode: EvalMode,
    pub config_path: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    pub bash: PathBuf,
    pub task_timeout: Duration,
    pub verify_timeout: Duration,
}

impl Default for EvalOptions {
    fn default() -> Self {
        Self {
            mode: EvalMode::Agent,
            config_path: None,
            output_dir: None,
            bash: default_bash(),
            task_timeout: Duration::from_secs(1800),
            verify_timeout: Duration::from_secs(600),
        }
    }
}

impl EvalOptions {
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            mode: std::env::var("XAIM_EVAL_MODE")
                .unwrap_or_else(|_| "agent".into())
                .parse()?,
            config_path: std::env::var_os("XAIM_EVAL_CONFIG").map(PathBuf::from),
            output_dir: std::env::var_os("XAIM_EVAL_OUTPUT").map(PathBuf::from),
            bash: std::env::var_os("EVAL_BASH")
                .map(PathBuf::from)
                .unwrap_or_else(default_bash),
            ..Self::default()
        })
    }

    pub fn report_dir(&self, tasks_dir: &Path) -> PathBuf {
        self.output_dir
            .clone()
            .unwrap_or_else(|| tasks_dir.parent().unwrap_or(tasks_dir).join("reports/rust"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub id: String,
    pub category: String,
    pub language: String,
    pub passed: bool,
    pub duration_ms: u64,
    pub tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_calls: u64,
    pub task_status: String,
    pub verify_status: String,
    pub task_log: String,
    pub verify_log: String,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EvalReport {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub avg_duration_ms: u64,
    pub avg_tokens: u64,
    pub details: Vec<TaskResult>,
    pub mode: String,
    pub errors: Vec<String>,
}

impl EvalReport {
    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.passed as f64 / self.total as f64
        }
    }
}

#[allow(dead_code)]
pub async fn run_eval(tasks_dir: &Path) -> EvalReport {
    match EvalOptions::from_env() {
        Ok(options) => run_eval_with_options(tasks_dir, &options).await,
        Err(error) => EvalReport {
            total: 0,
            passed: 0,
            failed: 0,
            avg_duration_ms: 0,
            avg_tokens: 0,
            details: vec![],
            mode: "agent".into(),
            errors: vec![error],
        },
    }
}

pub async fn run_eval_with_options(tasks_dir: &Path, options: &EvalOptions) -> EvalReport {
    run_with_provider(tasks_dir, options, None).await
}

async fn run_with_provider(
    tasks_dir: &Path,
    options: &EvalOptions,
    injected: Option<Arc<dyn ModelProvider>>,
) -> EvalReport {
    let mut errors = Vec::new();
    let dirs = match task_directories(tasks_dir) {
        Ok(dirs) => dirs,
        Err(error) => {
            errors.push(format!("Read tasks directory: {error}"));
            vec![]
        }
    };
    if dirs.is_empty() && errors.is_empty() {
        errors.push("No evaluation tasks found".into());
    }
    let connection = if options.mode == EvalMode::Agent {
        load_agent(options, injected)
    } else {
        Ok(None)
    };
    let mut details = Vec::new();
    for dir in dirs {
        let result = run_one(&dir, options, &connection).await;
        println!(
            "{} {} ({} ms, {} tokens)",
            if result.passed { "PASS" } else { "FAIL" },
            result.id,
            result.duration_ms,
            result.tokens
        );
        details.push(result);
    }
    let mut report = summarize(details, options.mode, errors);
    if let Err(error) = write_reports(&options.report_dir(tasks_dir), &report) {
        report.errors.push(format!("Write reports: {error}"));
    }
    report
}

type AgentConnection = Option<(OrchestratorConfig, Arc<dyn ModelProvider>)>;

fn load_agent(
    options: &EvalOptions,
    injected: Option<Arc<dyn ModelProvider>>,
) -> Result<AgentConnection, String> {
    if let Some(provider) = injected {
        return Ok(Some((
            OrchestratorConfig {
                candidate_count: 1,
                ..Default::default()
            },
            provider,
        )));
    }
    let config_path = options
        .config_path
        .as_ref()
        .ok_or("Agent mode requires --config or XAIM_EVAL_CONFIG (XAim Config JSON)")?;
    let data = fs::read(config_path).map_err(|error| format!("Read model config: {error}"))?;
    let mut config: types::Config =
        serde_json::from_slice(&data).map_err(|error| format!("Parse model config: {error}"))?;
    if let Ok(key) = std::env::var("XAIM_EVAL_API_KEY") {
        config.model.api_key = Some(key);
    }
    xaim_lib::config::model::validate(&config).map_err(|error| error.to_string())?;
    let config = OrchestratorConfig {
        model: config.model.into(),
        router: config.router,
        ..Default::default()
    };
    let provider =
        Arc::from(model::create_provider(&config.model).map_err(|error| error.to_string())?);
    Ok(Some((config, provider)))
}

async fn run_one(
    task_dir: &Path,
    options: &EvalOptions,
    connection: &Result<AgentConnection, String>,
) -> TaskResult {
    let started = Instant::now();
    let mut result = TaskResult {
        id: task_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        category: String::new(),
        language: String::new(),
        passed: false,
        duration_ms: 0,
        tokens: 0,
        input_tokens: 0,
        output_tokens: 0,
        model_calls: 0,
        task_status: "not-run".into(),
        verify_status: "not-run".into(),
        task_log: String::new(),
        verify_log: String::new(),
        error: None,
    };
    let outcome = async {
        let task_dir = task_dir.canonicalize().map_err(|error| error.to_string())?;
        let prompt = fs::read_to_string(task_dir.join("task.md"))
            .map_err(|error| format!("Read task.md: {error}"))?;
        if !task_dir.join("verify.sh").is_file() {
            return Err("Missing verify.sh".into());
        }
        let spec_path = task_dir.join("task.json");
        if spec_path.exists() {
            let spec: serde_json::Value =
                serde_json::from_slice(&fs::read(spec_path).map_err(|error| error.to_string())?)
                    .map_err(|error| format!("Parse task.json: {error}"))?;
            result.category = spec["category"].as_str().unwrap_or("").into();
            result.language = spec["language"].as_str().unwrap_or("").into();
        }
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let repo = temp.path().join("repo");
        copy_tree(&task_dir.join("repo"), &repo).map_err(|error| format!("Copy repo: {error}"))?;
        let preparation_error =
            if options.mode == EvalMode::Agent && matches!(connection, Ok(Some(_))) {
                let mut command = Command::new("node");
                command
                    .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/eval/prepare.mjs"))
                    .arg(bash_path(&task_dir))
                    .arg(bash_path(&repo));
                let prepared = execute(command, &repo, options.task_timeout).await;
                result.task_log.push_str(&prepared.log);
                prepared.error
            } else {
                None
            };
        let task_error = if let Some(error) = preparation_error {
            result.task_status = "failed".into();
            Some(format!("Prepare task: {error}"))
        } else {
            match options.mode {
                EvalMode::Baseline => {
                    result.task_status = "baseline".into();
                    None
                }
                EvalMode::Reference => {
                    let mut command = Command::new("git");
                    command
                        .arg("apply")
                        .arg("--whitespace=error")
                        .arg(bash_path(&task_dir.join("expected.diff")));
                    let output = execute(command, &repo, options.task_timeout).await;
                    result.task_status = if output.success { "passed" } else { "failed" }.into();
                    result.task_log = output.log;
                    output.error
                }
                EvalMode::Agent => match connection {
                    Err(error) => {
                        result.task_status = "failed".into();
                        Some(error.clone())
                    }
                    Ok(Some((config, provider))) => {
                        let counted = Arc::new(CountingProvider::new(provider.clone()));
                        let sink = Arc::new(CollectSink::default());
                        let run = run_agent(
                            &repo,
                            &prompt,
                            config.clone(),
                            counted.clone(),
                            sink.clone(),
                        );
                        let outcome = tokio::time::timeout(options.task_timeout, run).await;
                        result.input_tokens = counted.input.load(Ordering::Relaxed);
                        result.output_tokens = counted.output.load(Ordering::Relaxed);
                        result.tokens = result.input_tokens.saturating_add(result.output_tokens);
                        result.model_calls = counted.calls.load(Ordering::Relaxed);
                        result.task_log.push_str(&sink.0.lock().unwrap().join("\n"));
                        let error = match outcome {
                            Ok(Ok(())) => None,
                            Ok(Err(error)) => Some(error.to_string()),
                            Err(_) => Some(format!(
                                "Task timed out after {} seconds",
                                options.task_timeout.as_secs()
                            )),
                        };
                        result.task_status =
                            if error.is_none() { "passed" } else { "failed" }.into();
                        error
                    }
                    Ok(None) => Some("Agent provider unavailable".into()),
                },
            }
        };
        // Verification is independent of the agent outcome, so both failures remain visible.
        let mut command = Command::new(&options.bash);
        command
            .arg(bash_path(&task_dir.join("verify.sh")))
            .arg(bash_path(&repo));
        let verified = execute(command, &repo, options.verify_timeout).await;
        result.verify_status = if verified.success { "passed" } else { "failed" }.into();
        result.verify_log = verified.log;
        result.passed = task_error.is_none() && verified.success;
        match (task_error, verified.error) {
            (None, None) => Ok(()),
            (Some(task), Some(verify)) => Err(format!("Task: {task}; Verify: {verify}")),
            (Some(error), None) | (None, Some(error)) => Err(error),
        }
    }
    .await;
    if let Err(error) = outcome {
        result.error = Some(error);
    }
    result.duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    result
}

async fn run_agent(
    repo: &Path,
    prompt: &str,
    config: OrchestratorConfig,
    provider: Arc<dyn ModelProvider>,
    sink: Arc<dyn EventSink>,
) -> XaimResult<()> {
    let store = Arc::new(store::Store::open_in_memory()?);
    let project_id =
        store::project::insert_project(&store.conn(), &repo.to_string_lossy(), "eval")?;
    xaim_lib::git::ops::commit(repo, "Evaluation baseline")?;
    let orchestrator = Orchestrator::with_sink(store, config, sink, provider);
    let task_id = orchestrator.run_task(project_id, prompt.into()).await?;
    orchestrator.confirm_task(task_id).await
}

struct CountingProvider {
    inner: Arc<dyn ModelProvider>,
    input: AtomicU64,
    output: AtomicU64,
    calls: AtomicU64,
}
impl CountingProvider {
    fn new(inner: Arc<dyn ModelProvider>) -> Self {
        Self {
            inner,
            input: AtomicU64::new(0),
            output: AtomicU64::new(0),
            calls: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl ModelProvider for CountingProvider {
    async fn complete(&self, req: CompletionRequest) -> XaimResult<CompletionResponse> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let response = self.inner.complete(req).await?;
        self.input
            .fetch_add(u64::from(response.usage.input_tokens), Ordering::Relaxed);
        self.output
            .fetch_add(u64::from(response.usage.output_tokens), Ordering::Relaxed);
        Ok(response)
    }
    async fn stream(
        &self,
        _req: CompletionRequest,
    ) -> XaimResult<Pin<Box<dyn Stream<Item = XaimResult<String>> + Send>>> {
        Err(XaimError::Model(
            "Evaluation requires complete responses with token usage".into(),
        ))
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn max_context(&self) -> usize {
        self.inner.max_context()
    }
}

#[derive(Default)]
struct CollectSink(Mutex<Vec<String>>);
impl EventSink for CollectSink {
    fn emit(&self, event: XaimEvent) -> XaimResult<()> {
        self.0.lock().unwrap().push(serde_json::to_string(&event)?);
        Ok(())
    }
}

struct Execution {
    success: bool,
    log: String,
    error: Option<String>,
}

async fn execute(mut command: Command, repo: &Path, limit: Duration) -> Execution {
    command
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Execution {
                success: false,
                log: String::new(),
                error: Some(format!("Spawn command: {error}")),
            }
        }
    };
    #[cfg(unix)]
    let pid = child.id();
    #[cfg(windows)]
    let job = match process_job::ProcessJob::attach(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill().await;
            return Execution {
                success: false,
                log: String::new(),
                error: Some(format!("Create process job: {error}")),
            };
        }
    };
    let (stdout, mut stdout_reader) = capture(child.stdout.take().unwrap());
    let (stderr, mut stderr_reader) = capture(child.stderr.take().unwrap());
    let outcome = tokio::time::timeout(limit, child.wait()).await;
    let needs_termination = outcome.is_err() || matches!(&outcome, Ok(Err(_)));
    let (success, mut error) = match outcome {
        Ok(Ok(status)) => (
            status.success(),
            (!status.success()).then(|| format!("Exit status {status}")),
        ),
        Ok(Err(error)) => (false, Some(format!("Wait command: {error}"))),
        Err(_) => (
            false,
            Some(format!("Command timed out after {} ms", limit.as_millis())),
        ),
    };
    if needs_termination {
        #[cfg(unix)]
        if let Some(pid) = pid {
            terminate_tree(pid).await;
        }
        #[cfg(windows)]
        drop(job);
        let _ = child.kill().await;
    } else {
        #[cfg(windows)]
        drop(job);
    }
    let readers = async {
        (&mut stdout_reader).await?;
        (&mut stderr_reader).await?;
        Ok::<_, tokio::task::JoinError>(())
    };
    if tokio::time::timeout(Duration::from_secs(2), readers)
        .await
        .is_err()
    {
        stdout_reader.abort();
        stderr_reader.abort();
        error.get_or_insert_with(|| "Command output pipes did not close".into());
    }
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&stdout.lock().unwrap()),
        String::from_utf8_lossy(&stderr.lock().unwrap())
    );
    Execution {
        success: success && error.is_none(),
        log,
        error,
    }
}

fn capture<R: AsyncRead + Unpin + Send + 'static>(
    mut reader: R,
) -> (Arc<Mutex<Vec<u8>>>, tokio::task::JoinHandle<()>) {
    let output = Arc::new(Mutex::new(Vec::new()));
    let captured = output.clone();
    let task = tokio::spawn(async move {
        let mut buffer = [0u8; 8192];
        // Drain the entire pipe while retaining a bounded tail for diagnostics.
        while let Ok(size) = reader.read(&mut buffer).await {
            if size == 0 {
                break;
            }
            let mut output = captured.lock().unwrap();
            output.extend_from_slice(&buffer[..size]);
            if output.len() > 1_048_576 {
                let excess = output.len() - 1_048_576;
                output.drain(..excess);
            }
        }
    });
    (output, task)
}

#[cfg(unix)]
async fn terminate_tree(pid: u32) {
    let mut kill = {
        let mut command = Command::new("kill");
        command.args(["-KILL", "--", &format!("-{pid}")]);
        command
    };
    {
        kill.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), kill.status()).await;
    }
}

fn default_bash() -> PathBuf {
    #[cfg(windows)]
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        let bash = PathBuf::from(program_files).join("Git/bin/bash.exe");
        if bash.is_file() {
            return bash;
        }
    }
    PathBuf::from("bash")
}

fn bash_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    path.replace('\\', "/")
}

fn task_directories(tasks_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(tasks_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some(
                ".git"
                    | "node_modules"
                    | "dist"
                    | "target"
                    | "__pycache__"
                    | ".venv"
                    | ".pytest_cache"
                    | "build"
            )
        ) {
            continue;
        }
        let target = destination.join(name);
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(io::Error::other(format!(
                "Fixture symlink not supported: {}",
                entry.path().display()
            )));
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn summarize(details: Vec<TaskResult>, mode: EvalMode, errors: Vec<String>) -> EvalReport {
    let total = details.len();
    let passed = details.iter().filter(|task| task.passed).count();
    let average = |value: fn(&TaskResult) -> u64| -> u64 {
        if total == 0 {
            0
        } else {
            (details
                .iter()
                .map(|task| u128::from(value(task)))
                .sum::<u128>()
                / total as u128)
                .min(u64::MAX as u128) as u64
        }
    };
    EvalReport {
        total,
        passed,
        failed: total - passed,
        avg_duration_ms: average(|task| task.duration_ms),
        avg_tokens: average(|task| task.tokens),
        details,
        mode: mode.to_string(),
        errors,
    }
}

fn write_reports(output_dir: &Path, report: &EvalReport) -> io::Result<()> {
    fs::create_dir_all(output_dir)?;
    let mut json = serde_json::to_vec_pretty(report).map_err(io::Error::other)?;
    json.push(b'\n');
    fs::write(output_dir.join(format!("{}.json", report.mode)), json)?;
    fs::write(
        output_dir.join(format!("{}.md", report.mode)),
        markdown_report(report),
    )
}

fn cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn markdown_report(report: &EvalReport) -> String {
    let mut text = format!("# XAim 评测报告\n\n模式：{}\n\n| 指标 | 值 |\n| --- | ---: |\n| 总任务 | {} |\n| 通过 | {} |\n| 失败 | {} |\n| 成功率 | {:.2}% |\n| 平均耗时（ms） | {} |\n| 平均 token | {} |\n\ntoken 为模型响应 usage 的输入与输出之和；未返回 usage 时为 0。平均值包含全部任务。reference/baseline 不调用模型，不能作为 XAim 准确率。\n", report.mode, report.total, report.passed, report.failed, report.success_rate() * 100.0, report.avg_duration_ms, report.avg_tokens);
    if !report.errors.is_empty() {
        text.push_str("\n## 运行错误\n\n");
        for error in &report.errors {
            text.push_str(&format!("- {}\n", cell(error)));
        }
    }
    let mut categories: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for task in &report.details {
        let count = categories.entry(&task.category).or_default();
        count.1 += 1;
        count.0 += usize::from(task.passed);
    }
    text.push_str("\n## 分类结果\n\n| 类别 | 通过 / 总数 |\n| --- | ---: |\n");
    for (category, (passed, total)) in categories {
        text.push_str(&format!("| {} | {passed}/{total} |\n", cell(category)));
    }
    text.push_str("\n## 任务明细\n\n| 任务 | 类别 | 执行 | 验证 | 结果 | 耗时 ms | token | 错误 |\n| --- | --- | --- | --- | --- | ---: | ---: | --- |\n");
    for task in &report.details {
        text.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            cell(&task.id),
            cell(&task.category),
            task.task_status,
            task.verify_status,
            if task.passed { "通过" } else { "失败" },
            task.duration_ms,
            task.tokens,
            cell(task.error.as_deref().unwrap_or(""))
        ));
    }
    text.push_str("\n## 失败日志\n");
    for task in report.details.iter().filter(|task| !task.passed) {
        text.push_str(&format!(
            "\n### {}\n\n执行：\n\n~~~~text\n{}\n~~~~\n\n验证：\n\n~~~~text\n{}\n~~~~\n",
            cell(&task.id),
            task.task_log.replace("~~~~", "~ ~ ~ ~"),
            task.verify_log.replace("~~~~", "~ ~ ~ ~")
        ));
    }
    text
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
