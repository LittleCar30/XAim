//! Real development operations. Permission comes from the task entry point,
//! never from a model argument. Working-directory checks are not an OS sandbox.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

use crate::agent::workspace::safe_path;
use crate::errors::{Result, XaimError};
use crate::verify::process::ProcessTree;

const CAPTURE_BYTES: usize = 24_000;
const PROGRAMS: &[&str] = &[
    "git",
    "node",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "cargo",
    "rustc",
    "go",
    "python",
    "python3",
    "pytest",
    "make",
    "tsc",
    "eslint",
    "powershell",
    "pwsh",
    "sh",
    "bash",
];

pub fn definitions() -> Vec<Value> {
    let definition = |name: &str, description: &str, properties: Value, required: Value| {
        json!({"type":"function","function":{"name":name,"description":description,
            "parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}})
    };
    let command = json!({"program":{"type":"string"},"args":{"type":"array","items":{"type":"string"}},
        "cwd":{"type":"string","description":"任务副本内相对目录，默认 ."},
        "timeout_sec":{"type":"integer","minimum":1,"maximum":3600}});
    vec![
        definition("run_command", "按本轮命令授权实际运行开发命令，用于安装依赖、自定义测试、构建和诊断。program 与 args 分开传递，不支持命令串或 shell -c。Git 写操作须用专用工具。输出包含实际退出码；失败不代表执行成功。", command.clone(), json!(["program","args"])),
        definition("start_process", "启动开发服务，返回受管理的 process_id；不表示服务已就绪，必须 read_process 查看输出、状态，并按需要探测地址。最多运行 1 小时，可 stop_process；退出应用会停止。", command, json!(["program","args"])),
        definition("read_process", "读取本项目由 XAim 启动的进程状态和输出。", json!({"process_id":{"type":"string"}}), json!(["process_id"])),
        definition("stop_process", "停止本项目由 XAim 启动的进程及其子进程。", json!({"process_id":{"type":"string"}}), json!(["process_id"])),
        definition("clone_repository", "真实克隆公开 HTTPS Git 仓库并将完整目录交付到原项目，保留 .git 和二进制文件。path 是原项目内尚不存在的相对目录；. 表示空的原项目根。不得替换已有仓库。应在文件编辑前执行。支持可选递归子模块，LFS 指针会尝试拉取并核对。", json!({"url":{"type":"string"},"path":{"type":"string"},"recursive":{"type":"boolean"}}), json!(["url","path"])),
        definition("report_blocked", "缺少必要工具、授权、环境或用户信息，无法继续原始目标时提交具体原因与未完成事项并结束。不能生成替代脚本冒充操作完成。", json!({"reason":{"type":"string"},"pending":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":16}}), json!(["reason","pending"])),
    ]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandArgs {
    pub program: String,
    pub args: Vec<String>,
    #[serde(default = "default_cwd")]
    pub cwd: String,
    pub timeout_sec: Option<u64>,
}
fn default_cwd() -> String {
    ".".into()
}

pub fn redact(text: &str) -> String {
    static SECRET: OnceLock<regex::Regex> = OnceLock::new();
    SECRET.get_or_init(|| regex::Regex::new(r"(?i)(bearer\s+\S+|sk-[a-z0-9_-]{12,}|(?:api[_-]?key|access[_-]?token|password)\s*[:=]\s*[^\s,;]+|https?://[^/@\s]+:[^/@\s]+@)").unwrap())
        .replace_all(text, "[REDACTED]").into_owned()
}

fn directory(root: &Path, path: &str) -> Result<PathBuf> {
    let candidate = if path == "." {
        root.to_path_buf()
    } else {
        safe_path(root, path)?
    };
    let real = candidate.canonicalize()?;
    if !real.starts_with(root.canonicalize()?) || !real.is_dir() {
        return Err(XaimError::Security("命令工作目录必须在任务副本内".into()));
    }
    Ok(real)
}

pub(crate) fn prepare_dependencies(args: &CommandArgs, root: &Path, project: &Path) -> Result<()> {
    if !matches!(
        args.program.as_str(),
        "npm" | "pnpm" | "yarn" | "node" | "npx" | "tsc" | "eslint"
    ) || args
        .args
        .first()
        .is_some_and(|s| matches!(s.as_str(), "install" | "ci" | "add"))
    {
        return Ok(());
    }
    let candidate = directory(root, &args.cwd)?;
    if let Ok(original) = directory(project, &args.cwd) {
        crate::verify::environment::prepare_command_dependencies(&candidate, &original)?;
    }
    Ok(())
}

pub(crate) fn validate(args: &CommandArgs, root: &Path) -> Result<(PathBuf, PathBuf)> {
    if args.args.len() > 128
        || args
            .args
            .iter()
            .any(|s| s.len() > 16_000 || s.contains('\0'))
    {
        return Err(XaimError::Security("命令参数超出限制".into()));
    }
    let cwd = directory(root, &args.cwd)?;
    let program = if PROGRAMS.contains(&args.program.as_str()) {
        crate::verify::runner::resolve_program(&args.program)
    } else if args.program.starts_with(".venv/") || args.program.starts_with("venv/") {
        let relative = Path::new(&args.program);
        if relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(XaimError::Security("虚拟环境解释器路径无效".into()));
        }
        let p = root.join(relative);
        // Standard Unix virtualenv interpreters are symlinks to system Python.
        // The environment directory itself must remain inside the candidate.
        let environment = root.join(relative.components().next().unwrap().as_os_str());
        let file = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        if !environment
            .canonicalize()?
            .starts_with(root.canonicalize()?)
            || !matches!(file, "python" | "python3" | "python.exe")
        {
            return Err(XaimError::Security(
                "仅允许项目虚拟环境的 Python 解释器".into(),
            ));
        }
        p
    } else {
        return Err(XaimError::Blocked(format!(
            "开发命令 {} 尚未支持；请使用已接入的工具",
            args.program
        )));
    };
    if args.program == "git" {
        let sub = args.args.first().map(String::as_str).unwrap_or_default();
        if !matches!(
            sub,
            "status" | "log" | "show" | "diff" | "rev-parse" | "ls-files" | "remote" | "--version"
        ) || args.args.iter().any(|s| {
            s.starts_with("--output") || s.starts_with("--ext-diff") || s.starts_with("--textconv")
        }) || sub == "remote"
            && args
                .args
                .iter()
                .skip(1)
                .any(|s| !matches!(s.as_str(), "-v" | "get-url" | "origin"))
        {
            return Err(XaimError::Blocked(
                "此工具仅允许 Git 查询；克隆请使用 clone_repository，commit/reset 等写操作尚未接入"
                    .into(),
            ));
        }
    }
    if matches!(args.program.as_str(), "powershell" | "pwsh" | "bash" | "sh") {
        let script = if matches!(args.program.as_str(), "powershell" | "pwsh") {
            if args
                .args
                .first()
                .is_none_or(|s| !s.eq_ignore_ascii_case("-File"))
            {
                None
            } else {
                args.args.get(1)
            }
        } else {
            args.args.first().filter(|s| !s.starts_with('-'))
        };
        let script = script.ok_or_else(|| {
            XaimError::Security("Shell 仅支持运行项目脚本文件，不接受命令串".into())
        })?;
        let script = safe_path(&cwd, script)?;
        if !script.is_file() {
            return Err(XaimError::Blocked("项目脚本不存在".into()));
        }
    }
    if (args.args.iter().any(|s| {
        s == "global"
            || s == "--system"
            || s.starts_with("--global")
            || s.starts_with("-g")
            || s == "--location=global"
    }) || args.args.windows(2).any(|w| w == ["--location", "global"]))
        && matches!(args.program.as_str(), "npm" | "pnpm" | "yarn")
    {
        return Err(XaimError::Security(
            "不允许全局安装，依赖应安装到任务项目".into(),
        ));
    }
    if args.program.starts_with("python")
        && args.args.windows(2).any(|w| w == ["-m", "pip"])
        && args.args.iter().any(|s| s == "install")
    {
        return Err(XaimError::Blocked(
            "请先 python -m venv .venv，再用项目 .venv 的 Python 安装依赖，避免写入全局环境".into(),
        ));
    }
    Ok((program, cwd))
}

fn command(program: &Path, args: &[String], cwd: &Path) -> Command {
    let mut command = Command::new(program);
    crate::verify::process::configure(&mut command);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // No interactive authentication or inherited Git helpers/hooks.
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    command
}

fn git_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(local) = path.strip_prefix(r"\\?\") {
            return local.to_owned();
        }
    }
    path.into_owned()
}

async fn capture<R: AsyncRead + Unpin>(
    mut pipe: R,
    shared: Option<Arc<Mutex<Vec<u8>>>>,
) -> Vec<u8> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 4096];
    while let Ok(n) = pipe.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        output.extend_from_slice(&chunk[..n]);
        if output.len() > CAPTURE_BYTES {
            output.drain(..output.len() - CAPTURE_BYTES);
        }
        if let Some(shared) = &shared {
            let mut buffer = shared.lock().unwrap();
            buffer.extend_from_slice(&chunk[..n]);
            if buffer.len() > CAPTURE_BYTES {
                let discard = buffer.len() - CAPTURE_BYTES;
                buffer.drain(..discard);
            }
        }
    }
    output
}

pub async fn execute(
    args: CommandArgs,
    root: &Path,
    cancel: &AtomicBool,
    deadline: Option<Instant>,
) -> Result<Value> {
    if cancel.load(Ordering::SeqCst) {
        return Err(XaimError::Cancelled);
    }
    let (program, cwd) = validate(&args, root)?;
    let start = Instant::now();
    let child = command(&program, &args.args, &cwd).spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            XaimError::Blocked(format!("{} 未安装或不可执行", args.program))
        } else {
            XaimError::Io(e)
        }
    })?;
    let seconds = args.timeout_sec.unwrap_or(300).clamp(1, 3600);
    let result = wait(
        child,
        cancel,
        deadline
            .unwrap_or(start + Duration::from_secs(seconds))
            .min(start + Duration::from_secs(seconds)),
        None,
    )
    .await?;
    Ok(
        json!({"program":args.program,"args":args.args.iter().map(|s| redact(s)).collect::<Vec<_>>(),"cwd":args.cwd,
        "exitCode":result.0,"stdout":redact(&result.1),"stderr":redact(&result.2),"durationMs":start.elapsed().as_millis() as u64,"success":result.0==Some(0)}),
    )
}

async fn wait(
    child: Child,
    cancel: &AtomicBool,
    deadline: Instant,
    buffer: Option<Arc<Mutex<Vec<u8>>>>,
) -> Result<(Option<i32>, String, String)> {
    let tree = ProcessTree::attach(&child)?;
    wait_attached(child, tree, cancel, deadline, buffer).await
}

async fn wait_attached(
    mut child: Child,
    tree: ProcessTree,
    cancel: &AtomicBool,
    deadline: Instant,
    buffer: Option<Arc<Mutex<Vec<u8>>>>,
) -> Result<(Option<i32>, String, String)> {
    let stdout = tokio::spawn(capture(child.stdout.take().unwrap(), buffer.clone()));
    let stderr = tokio::spawn(capture(child.stderr.take().unwrap(), buffer));
    let outcome = loop {
        if cancel.load(Ordering::SeqCst) {
            break Err(XaimError::Cancelled);
        }
        if Instant::now() >= deadline {
            break Err(XaimError::Timeout("开发命令超过执行时限".into()));
        }
        match tokio::time::timeout(Duration::from_millis(100), child.wait()).await {
            Ok(result) => break result.map(|s| s.code()).map_err(XaimError::Io),
            Err(_) => continue,
        }
    };
    if outcome.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    drop(tree);
    let out = String::from_utf8_lossy(&stdout.await.unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&stderr.await.unwrap_or_default()).into_owned();
    outcome.map(|code| (code, out, err))
}

#[derive(Default)]
struct ManagedProcess {
    project: PathBuf,
    cancel: AtomicBool,
    output: Arc<Mutex<Vec<u8>>>,
    terminal: Mutex<Option<Value>>,
}
static PROCESSES: OnceLock<Mutex<HashMap<String, Arc<ManagedProcess>>>> = OnceLock::new();
fn processes() -> &'static Mutex<HashMap<String, Arc<ManagedProcess>>> {
    PROCESSES.get_or_init(Mutex::default)
}

pub async fn start(args: CommandArgs, root: &Path, project: &Path) -> Result<Value> {
    let (program, cwd) = validate(&args, root)?;
    // Reserve a slot before spawning. Stopped records are retained for diagnosis.
    let mut all = processes().lock().unwrap();
    if all
        .values()
        .filter(|p| p.terminal.lock().unwrap().is_none())
        .count()
        >= 8
    {
        return Err(XaimError::Blocked(
            "已有 8 个开发进程，请先停止不需要的进程".into(),
        ));
    }
    let child = command(&program, &args.args, &cwd)
        .spawn()
        .map_err(|e| XaimError::Blocked(format!("无法启动 {}: {e}", args.program)))?;
    let tree = ProcessTree::attach(&child)?;
    let pid = child.id();
    let id = uuid::Uuid::new_v4().to_string();
    let managed = Arc::new(ManagedProcess {
        project: project.canonicalize()?,
        ..Default::default()
    });
    if all.len() >= 64 {
        // Bound retained history without discarding it on every start.
        if let Some(old) = all
            .iter()
            .find(|(_, p)| p.terminal.lock().unwrap().is_some())
            .map(|(id, _)| id.clone())
        {
            all.remove(&old);
        }
    }
    all.insert(id.clone(), managed.clone());
    drop(all);
    let lifetime = Duration::from_secs(args.timeout_sec.unwrap_or(3600).clamp(1, 3600));
    tokio::spawn(async move {
        let result = wait_attached(
            child,
            tree,
            &managed.cancel,
            Instant::now() + lifetime,
            Some(managed.output.clone()),
        )
        .await;
        *managed.terminal.lock().unwrap() = Some(match result {
            Ok((code, _, _)) => json!({"state":"exited","exitCode":code}),
            Err(e) => json!({"state":"stopped","reason":e.to_string()}),
        });
    });
    Ok(
        json!({"process_id":id,"pid":pid,"state":"started","program":args.program,"args":args.args.iter().map(|s|redact(s)).collect::<Vec<_>>(),"cwd":args.cwd,"ready":false,"lifetimeSec":lifetime.as_secs()}),
    )
}

pub fn read_process(id: &str, project: &Path, stop: bool) -> Result<Value> {
    let all = processes().lock().unwrap();
    let p = all
        .get(id)
        .ok_or_else(|| XaimError::Blocked("进程不存在或应用已重启".into()))?;
    if p.project != project.canonicalize()? {
        return Err(XaimError::Security("进程不属于当前项目".into()));
    }
    if stop {
        p.cancel.store(true, Ordering::SeqCst);
    }
    let status = p
        .terminal
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| json!({"state":if stop {"stopping"} else {"running"}}));
    Ok(
        json!({"process_id":id,"status":status,"output":redact(&String::from_utf8_lossy(&p.output.lock().unwrap()))}),
    )
}

pub async fn stop_process(id: &str, project: &Path) -> Result<Value> {
    read_process(id, project, true)?;
    for _ in 0..100 {
        let status = read_process(id, project, false)?;
        if matches!(
            status["status"]["state"].as_str(),
            Some("stopped" | "exited")
        ) {
            return Ok(status);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(XaimError::Blocked(
        "已请求停止，但尚未确认进程结束；请继续读取进程状态".into(),
    ))
}
pub fn shutdown() {
    for p in processes().lock().unwrap().values() {
        p.cancel.store(true, Ordering::SeqCst);
    }
}

/// Failed/cancelled turns must not leave their newly launched services behind.
#[derive(Default)]
pub struct TaskProcesses {
    ids: Vec<String>,
    keep: bool,
}
impl TaskProcesses {
    pub fn register(&mut self, result: &Value) {
        if let Some(id) = result["process_id"].as_str() {
            self.ids.push(id.into());
        }
    }
    pub fn complete(&mut self) {
        self.keep = true;
    }
}
impl Drop for TaskProcesses {
    fn drop(&mut self) {
        if !self.keep {
            let all = processes().lock().unwrap();
            for id in &self.ids {
                if let Some(p) = all.get(id) {
                    p.cancel.store(true, Ordering::SeqCst);
                }
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloneArgs {
    pub url: String,
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

pub async fn clone_repository(
    args: CloneArgs,
    project: &Path,
    cancel: &AtomicBool,
    deadline: Option<Instant>,
) -> Result<Value> {
    clone_repository_with_network(args, project, cancel, deadline, &Default::default()).await
}

pub(crate) fn validate_clone_url(address: &str) -> Result<()> {
    let url =
        reqwest::Url::parse(address).map_err(|_| XaimError::Security("仓库 URL 无效".into()))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(XaimError::Security(
            "仅支持不含凭据的公开 HTTPS 仓库 URL".into(),
        ));
    }
    Ok(())
}

pub async fn clone_repository_with_network(
    args: CloneArgs,
    project: &Path,
    cancel: &AtomicBool,
    deadline: Option<Instant>,
    network: &crate::config::execution::GitNetwork,
) -> Result<Value> {
    validate_clone_url(&args.url)?;
    network.validate()?;
    clone_to_project(args, project, cancel, deadline, network).await
}

async fn git(
    args: &[String],
    root: &Path,
    cancel: &AtomicBool,
    deadline: Option<Instant>,
    network: &crate::config::execution::GitNetwork,
) -> Result<Value> {
    if cancel.load(Ordering::SeqCst) {
        return Err(XaimError::Cancelled);
    }
    // Internal Git commands bypass the model's read-only Git argument policy.
    let start = Instant::now();
    // Proxy options must precede the subcommand; this configuration is local to
    // the process and inherited by submodule/LFS children where applicable.
    let mut git_command = command(&crate::verify::runner::resolve_program("git"), &[], root);
    super::git_network::configure_remote(&mut git_command, network, args);
    let child = git_command
        .args(args)
        .spawn()
        .map_err(|e| XaimError::Blocked(format!("无法运行 Git: {e}")))?;
    let (code, stdout, stderr) = wait(
        child,
        cancel,
        deadline.unwrap_or(start + Duration::from_secs(600)),
        None,
    )
    .await?;
    if code != Some(0) {
        return Err(XaimError::Blocked(format!(
            "Git 操作失败（退出码 {code:?}）: {}\n如为连接错误，请检查项目执行权限中的 Git 网络设置，选择系统代理或自定义代理后重试。",
            redact(&stderr)
        )));
    }
    Ok(json!({"exitCode":code,"stdout":stdout,"stderr":redact(&stderr)}))
}

async fn clone_to_project(
    args: CloneArgs,
    project: &Path,
    cancel: &AtomicBool,
    deadline: Option<Instant>,
    network: &crate::config::execution::GitNetwork,
) -> Result<Value> {
    let project = project.canonicalize()?;
    let target = if args.path == "." {
        project.clone()
    } else {
        safe_path(&project, &args.path)?
    };
    let in_place = target == project;
    if in_place {
        if std::fs::read_dir(&target)?.next().is_some() {
            return Err(XaimError::Blocked(
                "原项目根目录非空，不能原地克隆；请选择尚不存在的子目录".into(),
            ));
        }
    } else if target.exists() {
        return Err(XaimError::Blocked(
            "克隆目标已存在，不会覆盖或重置已有目录".into(),
        ));
    }
    let parent = target
        .parent()
        .ok_or_else(|| XaimError::Security("克隆目标无效".into()))?;
    if !parent.is_dir() {
        return Err(XaimError::Blocked("目标父目录不存在".into()));
    }
    let stage = tempfile::Builder::new()
        .prefix(".xaim-clone-")
        .tempdir_in(parent)?;
    let repo_dir = stage.path().join("repository");
    let mut clone = vec!["clone".into(), "--no-local".into()];
    if args.recursive {
        clone.push("--recurse-submodules".into());
    }
    clone.extend(["--".into(), args.url.clone(), git_path(&repo_dir)]);
    git(&clone, &project, cancel, deadline, network).await?;
    let mut pointers = lfs_pointers(&repo_dir)?;
    if pointers > 0 {
        git(
            &["lfs".into(), "install".into(), "--local".into()],
            &repo_dir,
            cancel,
            deadline,
            network,
        )
        .await?;
        git(
            &["lfs".into(), "pull".into()],
            &repo_dir,
            cancel,
            deadline,
            network,
        )
        .await?;
        pointers = lfs_pointers(&repo_dir)?;
        if pointers > 0 {
            return Err(XaimError::Blocked(
                "仓库仍包含未拉取的 LFS 内容，未交付目标目录".into(),
            ));
        }
    }
    let evidence = repository_evidence(&repo_dir, &args.url)?;
    let submodules = git(
        &["submodule".into(), "status".into()],
        &repo_dir,
        cancel,
        deadline,
        network,
    )
    .await?;
    if args.recursive
        && submodules["stdout"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .any(|l| l.starts_with('-') || l.starts_with('+') || l.starts_with('U'))
    {
        return Err(XaimError::Blocked(
            "子模块尚未完整就绪，未交付目标目录".into(),
        ));
    }
    if cancel.load(Ordering::SeqCst) {
        return Err(XaimError::Cancelled);
    }
    // Create-only delivery on the same filesystem. A race must never overwrite
    // an existing directory, including one created while Git was downloading.
    if in_place {
        if std::fs::read_dir(&target)?.next().is_some() {
            return Err(XaimError::Blocked(
                "克隆期间目标目录已变化，拒绝覆盖".into(),
            ));
        }
        std::fs::remove_dir(&target)?;
    }
    let delivered = rename_new(&repo_dir, &target);
    if let Err(e) = delivered {
        if in_place && !target.exists() {
            let _ = std::fs::create_dir(&target);
        }
        return Err(XaimError::Io(e));
    }
    Ok(
        json!({"success":true,"delivered":true,"url":args.url,"path":args.path,"absolutePath":target,
        "head":evidence["head"],"empty":evidence["empty"],"submodules":submodules["stdout"],"recursive":args.recursive,"lfsPointers":pointers}),
    )
}

/// Atomically refuse replacing a destination, including an empty directory
/// created by another process after preflight.
fn rename_new(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileW(old: *const u16, new: *const u16) -> i32;
        }
        let old: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let new: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileW(old.as_ptr(), new.as_ptr()) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn renameat2(
                oldfd: i32,
                old: *const std::ffi::c_char,
                newfd: i32,
                new: *const std::ffi::c_char,
                flags: u32,
            ) -> i32;
        }
        let old = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let new = std::ffi::CString::new(target.as_os_str().as_bytes())?;
        if unsafe { renameat2(-100, old.as_ptr(), -100, new.as_ptr(), 1) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn renamex_np(
                old: *const std::ffi::c_char,
                new: *const std::ffi::c_char,
                flags: u32,
            ) -> i32;
        }
        let old = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let new = std::ffi::CString::new(target.as_os_str().as_bytes())?;
        if unsafe { renamex_np(old.as_ptr(), new.as_ptr(), 4) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (source, target);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "create-only repository delivery is unavailable",
        ))
    }
}

pub fn repository_evidence(root: &Path, expected: &str) -> Result<Value> {
    let repo = git2::Repository::open(root)?;
    if repo.is_bare()
        || repo
            .workdir()
            .is_none_or(|p| p.canonicalize().ok() != root.canonicalize().ok())
    {
        return Err(XaimError::Blocked("目标不是预期的 Git 工作区".into()));
    }
    let origin = repo.find_remote("origin")?;
    let normalize = |s: &str| s.trim_end_matches('/').trim_end_matches(".git").to_owned();
    if origin
        .url()
        .is_none_or(|s| normalize(s) != normalize(expected))
    {
        return Err(XaimError::Blocked("仓库 origin 与请求不一致".into()));
    }
    let empty = repo.is_empty()?;
    let head = if empty {
        None
    } else {
        Some(repo.head()?.peel_to_commit()?.id().to_string())
    };
    Ok(json!({"origin":origin.url(),"head":head,"empty":empty}))
}

fn lfs_pointers(root: &Path) -> Result<usize> {
    fn walk(dir: &Path, count: &mut usize, visited: &mut usize) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_name() == ".git" {
                continue;
            }
            *visited += 1;
            if *visited > 100_000 {
                return Err(XaimError::Blocked("仓库文件过多，无法完成导入检查".into()));
            }
            let meta = std::fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                walk(&entry.path(), count, visited)?;
            } else if meta.len() <= 2048
                && std::fs::read(entry.path())?
                    .starts_with(b"version https://git-lfs.github.com/spec/v1\n")
            {
                *count += 1;
            }
        }
        Ok(())
    }
    let mut count = 0;
    walk(root, &mut count, &mut 0)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    #[ignore = "Requires public GitHub access through the user's system proxy"]
    async fn online_clone_uses_system_proxy_and_delivers_complete_repository() {
        let root = tempfile::tempdir().unwrap();
        let args = CloneArgs {
            url: "https://github.com/nyoungo/dsfree2api.git".into(),
            path: "repo".into(),
            recursive: false,
        };
        let approval = crate::agent::permissions::scope(
            "clone_repository",
            &serde_json::json!({"url":args.url,"path":args.path}),
            root.path(),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(approval.tool, "clone_repository");
        let result = clone_repository(
            args,
            root.path(),
            &AtomicBool::new(false),
            Some(Instant::now() + Duration::from_secs(60)),
        )
        .await
        .unwrap();
        assert_eq!(result["delivered"], true);
        assert!(result["head"].as_str().is_some_and(|head| !head.is_empty()));
        assert!(root.path().join("repo/.git").is_dir());
        assert!(root.path().join("repo/README.md").is_file());
        assert!(root.path().join("repo/go.mod").is_file());
    }

    async fn clone_to_project(
        args: CloneArgs,
        project: &Path,
        cancel: &AtomicBool,
        deadline: Option<Instant>,
    ) -> Result<Value> {
        super::clone_to_project(args, project, cancel, deadline, &Default::default()).await
    }

    fn cmd(program: &str, args: &[&str]) -> CommandArgs {
        CommandArgs {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: ".".into(),
            timeout_sec: Some(15),
        }
    }

    #[tokio::test]
    async fn development_commands_return_real_exit_codes_and_do_not_write_original() {
        let original = tempfile::tempdir().unwrap();
        let workspace = crate::agent::workspace::TaskWorkspace::snapshot(original.path()).unwrap();
        let result=execute(cmd("node", &["-e", "require('fs').writeFileSync('created.txt','actual'); console.log('ok'); console.error('failure'); process.exit(7)"]),workspace.root(),&AtomicBool::new(false),None).await.unwrap();
        assert_eq!(result["exitCode"], 7);
        assert_eq!(result["success"], false);
        assert!(result["stdout"].as_str().unwrap().contains("ok"));
        assert!(result["stderr"].as_str().unwrap().contains("failure"));
        assert!(!original.path().join("created.txt").exists());
        assert_eq!(
            fs::read_to_string(workspace.root().join("created.txt")).unwrap(),
            "actual"
        );
    }

    #[tokio::test]
    async fn project_commands_reuse_matching_dependencies_in_a_nested_candidate() {
        let original = tempfile::tempdir().unwrap();
        fs::create_dir_all(original.path().join("app/node_modules/fixture")).unwrap();
        fs::write(
            original.path().join("app/package.json"),
            "{\"name\":\"fixture\"}",
        )
        .unwrap();
        fs::write(
            original.path().join("app/node_modules/fixture/index.js"),
            "module.exports = 'dependency works'",
        )
        .unwrap();
        let workspace = crate::agent::workspace::TaskWorkspace::snapshot(original.path()).unwrap();
        let mut args = cmd("node", &["-e", "console.log(require('fixture'))"]);
        args.cwd = "app".into();
        prepare_dependencies(&args, workspace.root(), original.path()).unwrap();
        let result = execute(args, workspace.root(), &AtomicBool::new(false), None)
            .await
            .unwrap();
        assert_eq!(result["success"], true);
        assert!(result["stdout"]
            .as_str()
            .unwrap()
            .contains("dependency works"));
        assert!(workspace.changes().unwrap().files.is_empty());
        assert_eq!(
            fs::read_to_string(original.path().join("app/node_modules/fixture/index.js")).unwrap(),
            "module.exports = 'dependency works'"
        );
    }

    #[test]
    fn virtualenv_interpreter_is_allowed_without_opening_parent_paths() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".venv/Scripts")).unwrap();
        fs::write(root.path().join(".venv/Scripts/python.exe"), []).unwrap();
        assert!(validate(
            &cmd(
                ".venv/Scripts/python.exe",
                &["-m", "pip", "install", "-r", "requirements.txt"]
            ),
            root.path()
        )
        .is_ok());
        assert!(validate(&cmd(".venv/../../python.exe", &[]), root.path()).is_err());
    }

    #[tokio::test]
    async fn failed_turn_stops_new_services_and_retains_exit_records() {
        let root = tempfile::tempdir().unwrap();
        let result = start(
            cmd("node", &["-e", "setInterval(()=>{},1000)"]),
            root.path(),
            root.path(),
        )
        .await
        .unwrap();
        let id = result["process_id"].as_str().unwrap();
        let mut scope = TaskProcesses::default();
        scope.register(&result);
        drop(scope);
        for _ in 0..100 {
            if read_process(id, root.path(), false).unwrap()["status"]["state"] == "stopped" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            read_process(id, root.path(), false).unwrap()["status"]["state"],
            "stopped"
        );
        let next = start(
            cmd("node", &["-e", "setInterval(()=>{},1000)"]),
            root.path(),
            root.path(),
        )
        .await
        .unwrap();
        assert_eq!(
            read_process(id, root.path(), false).unwrap()["status"]["state"],
            "stopped"
        );
        assert_eq!(
            stop_process(next["process_id"].as_str().unwrap(), root.path())
                .await
                .unwrap()["status"]["state"],
            "stopped"
        );
    }

    #[tokio::test]
    async fn unauthorized_argument_forms_and_escape_are_rejected_before_spawn() {
        let root = tempfile::tempdir().unwrap();
        for command in [
            cmd("git", &["reset", "--hard"]),
            cmd("sh", &["-c", "echo unsafe"]),
            cmd("npm", &["install", "--global", "package"]),
            cmd("npm", &["install", "--global=true", "package"]),
            cmd("npm", &["install", "--location=global", "package"]),
            cmd("yarn", &["global", "add", "package"]),
            cmd("python", &["-m", "pip", "--isolated", "install", "package"]),
        ] {
            assert!(execute(command, root.path(), &AtomicBool::new(false), None)
                .await
                .is_err());
        }
        let mut outside = cmd("node", &["--version"]);
        outside.cwd = "..".into();
        assert!(execute(outside, root.path(), &AtomicBool::new(false), None)
            .await
            .is_err());
        let args = CloneArgs {
            url: "https://user:secret@example.com/repo.git".into(),
            path: "repo".into(),
            recursive: false,
        };
        assert!(matches!(
            clone_repository(args, root.path(), &AtomicBool::new(false), None).await,
            Err(XaimError::Security(_))
        ));
    }

    fn local_repo(root: &Path) {
        let repo = git2::Repository::init(root).unwrap();
        fs::write(root.join("README.md"), "real repository").unwrap();
        fs::write(root.join("asset.bin"), [0, 255, 1, 128]).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("README.md")).unwrap();
        index.add_path(Path::new("asset.bin")).unwrap();
        index.write().unwrap();
        let oid = index.write_tree().unwrap();
        let tree = repo.find_tree(oid).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &[])
            .unwrap();
    }

    #[tokio::test]
    async fn clone_delivers_git_metadata_binary_and_updates_candidate_baseline() {
        let source = tempfile::tempdir().unwrap();
        local_repo(source.path());
        let root = tempfile::tempdir().unwrap();
        let mut workspace = crate::agent::workspace::TaskWorkspace::snapshot(root.path()).unwrap();
        let url = source.path().to_string_lossy().into_owned();
        let result = clone_to_project(
            CloneArgs {
                url: url.clone(),
                path: "repo".into(),
                recursive: false,
            },
            root.path(),
            &AtomicBool::new(false),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result["delivered"], true);
        assert!(root.path().join("repo/.git").is_dir());
        assert_eq!(
            fs::read(root.path().join("repo/asset.bin")).unwrap(),
            vec![0, 255, 1, 128]
        );
        repository_evidence(&root.path().join("repo"), &url).unwrap();
        workspace.adopt_import(root.path()).unwrap();
        assert!(workspace.changes().unwrap().files.is_empty());
        assert!(workspace.root().join("repo/README.md").is_file());
        assert!(!workspace.root().join("repo/.git").exists());
        assert!(clone_to_project(
            CloneArgs {
                url,
                path: "repo".into(),
                recursive: false
            },
            root.path(),
            &AtomicBool::new(false),
            None
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn empty_repository_in_place_clone_and_failed_clone_preserve_target() {
        let source = tempfile::tempdir().unwrap();
        git2::Repository::init(source.path()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let url = source.path().to_string_lossy().into_owned();
        let result = clone_to_project(
            CloneArgs {
                url,
                path: ".".into(),
                recursive: false,
            },
            root.path(),
            &AtomicBool::new(false),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result["empty"], true);
        assert!(result["head"].is_null());
        let empty = tempfile::tempdir().unwrap();
        assert!(clone_to_project(
            CloneArgs {
                url: source.path().join("missing").to_string_lossy().into_owned(),
                path: ".".into(),
                recursive: false
            },
            empty.path(),
            &AtomicBool::new(false),
            None
        )
        .await
        .is_err());
        assert!(empty.path().is_dir());
        assert_eq!(fs::read_dir(empty.path()).unwrap().count(), 0);
        fs::write(empty.path().join("existing"), "keep").unwrap();
        assert!(clone_to_project(
            CloneArgs {
                url: "unused".into(),
                path: ".".into(),
                recursive: false
            },
            empty.path(),
            &AtomicBool::new(false),
            None
        )
        .await
        .is_err());
        assert_eq!(
            fs::read_to_string(empty.path().join("existing")).unwrap(),
            "keep"
        );
    }

    #[tokio::test]
    async fn lfs_pointer_without_lfs_content_blocks_delivery() {
        let source = tempfile::tempdir().unwrap();
        local_repo(source.path());
        let repo = git2::Repository::open(source.path()).unwrap();
        fs::write(source.path().join("pointer.bin"),"version https://git-lfs.github.com/spec/v1\noid sha256:0000000000000000000000000000000000000000000000000000000000000000\nsize 1\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("pointer.bin")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "pointer", &tree, &[&parent])
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        assert!(clone_to_project(
            CloneArgs {
                url: source.path().to_string_lossy().into_owned(),
                path: "repo".into(),
                recursive: false
            },
            root.path(),
            &AtomicBool::new(false),
            None
        )
        .await
        .is_err());
        assert!(!root.path().join("repo").exists());
    }

    #[tokio::test]
    async fn cancellation_and_timeout_terminate_long_commands() {
        let root = tempfile::tempdir().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            trigger.store(true, Ordering::SeqCst);
        });
        assert!(matches!(
            execute(
                cmd("node", &["-e", "setInterval(()=>{},1000)"]),
                root.path(),
                &cancel,
                None
            )
            .await,
            Err(XaimError::Cancelled)
        ));
        let mut timeout = cmd("node", &["-e", "setInterval(()=>{},1000)"]);
        timeout.timeout_sec = Some(1);
        assert!(matches!(
            execute(timeout, root.path(), &AtomicBool::new(false), None).await,
            Err(XaimError::Timeout(_))
        ));
    }

    #[tokio::test]
    async fn managed_service_can_be_read_and_stopped_by_its_project_only() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let started = start(
            cmd(
                "node",
                &[
                    "-e",
                    "console.log('service-ready');setInterval(()=>{},1000)",
                ],
            ),
            root.path(),
            root.path(),
        )
        .await
        .unwrap();
        let id = started["process_id"].as_str().unwrap();
        assert!(read_process(id, other.path(), true).is_err());
        for _ in 0..50 {
            if read_process(id, root.path(), false).unwrap()["output"]
                .as_str()
                .unwrap()
                .contains("service-ready")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(read_process(id, root.path(), false).unwrap()["output"]
            .as_str()
            .unwrap()
            .contains("service-ready"));
        read_process(id, root.path(), true).unwrap();
        for _ in 0..50 {
            if read_process(id, root.path(), false).unwrap()["status"]["state"] == "stopped" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("process did not stop");
    }

    #[test]
    fn atomic_delivery_never_replaces_existing_empty_directory() {
        let parent = tempfile::tempdir().unwrap();
        let source = parent.path().join("source");
        let target = parent.path().join("target");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("keep"), "source").unwrap();
        fs::create_dir(&target).unwrap();
        assert!(rename_new(&source, &target).is_err());
        assert!(source.join("keep").exists());
    }

    #[test]
    fn diagnostics_redact_obvious_credentials() {
        let output = redact(
            "Bearer secret-token password=secret https://user:pass@host sk-abcdefghijklmnop",
        );
        assert!(!output.contains("secret"));
        assert!(!output.contains("pass@"));
        assert!(!output.contains("sk-"));
    }
}
