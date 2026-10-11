//! Execution approvals are generated and enforced by the program, not the model.
use super::tool_loop::Recorder;
use crate::config::execution::{self, ExecutionMode, ExecutionScope, GitNetwork};
use crate::errors::{Result, XaimError};
use crate::events::XaimEvent;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub id: String,
    pub task_id: i64,
    pub project_id: i64,
    pub purpose: String,
    pub scope: ExecutionScope,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Decision {
    Once,
    Remember,
    Deny,
}

#[derive(Debug)]
struct Pending {
    request: PermissionRequest,
    sender: oneshot::Sender<Decision>,
}

pub struct PermissionSession {
    store: Arc<Store>,
    project_id: i64,
    allowed: Mutex<HashSet<String>>,
    pending: Mutex<Option<Pending>>,
    pub git_network: GitNetwork,
}

impl std::fmt::Debug for PermissionSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionSession")
            .field("project_id", &self.project_id)
            .finish_non_exhaustive()
    }
}

impl PermissionSession {
    pub fn new(store: Arc<Store>, project_id: i64) -> Result<Self> {
        let settings = execution::load(&store, project_id)?;
        Ok(Self {
            store,
            project_id,
            allowed: Mutex::new(HashSet::new()),
            pending: Mutex::new(None),
            git_network: settings.git_network,
        })
    }

    pub fn pending(&self) -> Option<PermissionRequest> {
        self.pending
            .lock()
            .unwrap()
            .as_ref()
            .map(|p| p.request.clone())
    }

    pub fn resolve(&self, task_id: i64, request_id: &str, decision: Decision) -> Result<()> {
        let mut pending = self.pending.lock().unwrap();
        let item = pending
            .as_ref()
            .filter(|p| p.request.id == request_id && p.request.task_id == task_id)
            .ok_or_else(|| XaimError::Config("授权请求已结束或不属于当前任务".into()))?;
        if item.sender.is_closed() {
            pending.take();
            return Err(XaimError::Config("任务已停止，不能再授权".into()));
        }
        pending
            .take()
            .unwrap()
            .sender
            .send(decision)
            .map_err(|_| XaimError::Config("任务已停止，不能再授权".into()))
    }

    pub fn revoke(&self, id: &str) {
        self.allowed.lock().unwrap().remove(id);
    }

    pub async fn authorize(
        &self,
        scope: ExecutionScope,
        purpose: &str,
        cancel: &AtomicBool,
        deadline: Option<Instant>,
        record: &Recorder<'_>,
    ) -> Result<()> {
        if cancel.load(Ordering::SeqCst) {
            return Err(XaimError::Cancelled);
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(XaimError::Timeout("执行授权已超过任务时限".into()));
        }
        let settings = execution::load(&self.store, self.project_id)?;
        if settings.mode == ExecutionMode::Disabled {
            return Err(XaimError::Blocked(
                "项目设置为不执行程序。请在项目执行权限中选择按需询问后重试".into(),
            ));
        }
        if self.allowed.lock().unwrap().contains(&scope.id)
            || settings.mode == ExecutionMode::Project
                && settings.grants.iter().any(|g| g.id == scope.id)
        {
            record.diagnostic(
                "executionAuthorization",
                json!({"scope":scope,"source":"approvedScope"}),
            )?;
            return Ok(());
        }
        let request = PermissionRequest {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: record.task_id,
            project_id: self.project_id,
            purpose: purpose.into(),
            scope: scope.clone(),
        };
        let (sender, mut receiver) = oneshot::channel();
        *self.pending.lock().unwrap() = Some(Pending {
            request: request.clone(),
            sender,
        });
        let _cleanup = PendingGuard(self);
        record.event(XaimEvent::PermissionRequested {
            task_id: record.task_id,
            request: request.clone(),
        })?;
        let decision = loop {
            tokio::select! {
                result = &mut receiver => break result.map_err(|_| XaimError::Cancelled)?,
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if cancel.load(Ordering::SeqCst) { return Err(XaimError::Cancelled); }
                    if deadline.is_some_and(|d| Instant::now() >= d) { return Err(XaimError::Timeout("等待执行授权超过任务时限".into())); }
                }
            }
        };
        // A response racing task cancellation cannot authorize an operation.
        if cancel.load(Ordering::SeqCst) {
            return Err(XaimError::Cancelled);
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(XaimError::Timeout("执行授权已超过任务时限".into()));
        }
        match decision {
            Decision::Deny => {}
            Decision::Once => {
                self.allowed.lock().unwrap().insert(scope.id);
            }
            Decision::Remember => execution::remember(&self.store, self.project_id, scope)?,
        }
        record.event(XaimEvent::PermissionResolved {
            task_id: record.task_id,
            request_id: request.id,
            decision,
        })?;
        if matches!(decision, Decision::Deny) {
            Err(XaimError::Cancelled)
        } else {
            Ok(())
        }
    }
}

struct PendingGuard<'a>(&'a PermissionSession);
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.0.pending.lock().unwrap().take();
    }
}

/// Exact tool arguments plus script/manifests digest. No wildcard grants.
pub fn scope(
    tool: &str,
    args: &Value,
    root: &Path,
    network: &GitNetwork,
) -> Result<ExecutionScope> {
    let canonical_root = root.canonicalize()?;
    let root = canonical_root.as_path();
    let args = if tool == "verify" {
        json!({"command":args["command"]})
    } else if tool == "clone_repository" {
        let clone: crate::tools::operations::CloneArgs = serde_json::from_value(args.clone())?;
        // Apply URL validation before presenting or storing anything.
        crate::tools::operations::validate_clone_url(&clone.url)?;
        let path = if clone.path == "." {
            root.to_path_buf()
        } else {
            super::workspace::safe_path(root, &clone.path)?
        };
        let relative = path
            .strip_prefix(root)
            .map_err(|_| XaimError::Security("克隆路径无效".into()))?;
        json!({"url":clone.url,"path":if relative.as_os_str().is_empty() {".".into()} else {relative.to_string_lossy().replace('\\',"/")},"recursive":clone.recursive})
    } else {
        let command: crate::tools::operations::CommandArgs = serde_json::from_value(args.clone())?;
        let (_, directory) = crate::tools::operations::validate(&command, root)?;
        let canonical = root.canonicalize()?;
        let relative = directory
            .strip_prefix(&canonical)
            .map_err(|_| XaimError::Security("命令路径无效".into()))?;
        json!({"program":command.program,"args":command.args,"cwd":if relative.as_os_str().is_empty() {".".into()} else {relative.to_string_lossy().replace('\\',"/")}})
    };
    let raw = args.to_string();
    if crate::tools::operations::redact(&raw) != raw {
        return Err(XaimError::Security(
            "执行参数包含凭据信息，请通过项目环境配置提供凭据".into(),
        ));
    }
    let mut digest = blake3::Hasher::new();
    digest.update(tool.as_bytes());
    digest.update(raw.as_bytes());
    digest.update(serde_json::to_string(network)?.as_bytes());
    let cwd = args["cwd"].as_str().unwrap_or(".");
    let base = if cwd == "." {
        root.to_path_buf()
    } else {
        super::workspace::safe_path(root, cwd)?
    };
    // Changes to package scripts, dependency locks or a directly executed script require reapproval.
    if tool != "clone_repository" {
        let mut inputs = vec![
            "package.json".to_string(),
            "package-lock.json".into(),
            "pnpm-lock.yaml".into(),
            "yarn.lock".into(),
            "Cargo.toml".into(),
            "Cargo.lock".into(),
            "go.mod".into(),
            "go.sum".into(),
            "pyproject.toml".into(),
            "requirements.txt".into(),
            "Makefile".into(),
        ];
        if tool == "verify" {
            inputs.extend(crate::tools::exec::split_command(
                args["command"].as_str().unwrap_or_default(),
            )?);
        }
        if let Some(items) = args["args"].as_array() {
            inputs.extend(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.starts_with('-'))
                    .map(str::to_owned),
            );
        }
        for input in inputs {
            if let Ok(path) = super::workspace::safe_path(&base, input.trim_start_matches("./")) {
                if path.is_file() {
                    digest.update(&(input.len() as u64).to_le_bytes());
                    digest.update(input.as_bytes());
                    let mut content = blake3::Hasher::new();
                    content.update_reader(std::fs::File::open(path)?)?;
                    digest.update(content.finalize().as_bytes());
                }
            }
        }
    }
    let command = if tool == "clone_repository" {
        format!(
            "git clone {}-- {} {}",
            if args["recursive"] == true {
                "--recurse-submodules "
            } else {
                ""
            },
            serde_json::to_string(args["url"].as_str().unwrap_or_default())?,
            serde_json::to_string(args["path"].as_str().unwrap_or_default())?
        )
    } else if tool == "verify" {
        args["command"].as_str().unwrap_or_default().to_string()
    } else {
        format!(
            "{} {}",
            args["program"].as_str().unwrap_or_default(),
            args["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| serde_json::to_string(v.as_str().unwrap_or_default()).unwrap())
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    let original = tool == "clone_repository" || args["program"] == "git";
    let location = if original {
        "原项目"
    } else {
        "任务工作区"
    };
    let installing = args["args"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|v| matches!(v.as_str(), Some("install" | "ci" | "add")))
    });
    Ok(ExecutionScope {
        id: digest.finalize().to_hex().to_string(),
        tool: tool.into(),
        command,
        location: format!(
            "{location} / {}",
            if tool == "clone_repository" {
                args["path"].as_str().unwrap_or(".")
            } else {
                cwd
            }
        ),
        network: if tool == "clone_repository" {
            "需要访问仓库网络".into()
        } else if installing {
            "通常需要联网下载依赖".into()
        } else {
            "程序及脚本可能访问网络（当前无网络隔离）".into()
        },
        impact: if tool == "clone_repository" {
            "创建完整仓库目录；不会覆盖已有目标"
        } else if tool == "start_process" {
            "启动开发服务，最长运行一小时；可停止"
        } else if original {
            "查询原项目 Git 状态"
        } else if installing {
            "在任务副本安装依赖，可能运行安装脚本；脚本以你的系统权限执行"
        } else {
            "在任务副本执行；脚本以你的系统权限运行，可访问本机文件"
        }
        .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repair::r#loop::EventSink;
    use crate::store;
    #[derive(Default)]
    struct Sink(Mutex<Vec<XaimEvent>>);
    impl EventSink for Sink {
        fn emit(&self, event: XaimEvent) -> Result<()> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }
    fn setup(root: &Path) -> (Arc<Store>, i64, i64) {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let conn = store.conn();
        let project =
            store::project::insert_project(&conn, root.to_str().unwrap(), "test").unwrap();
        let task = store::task::insert_task(&conn, project, "execute").unwrap();
        drop(conn);
        (store, project, task)
    }
    async fn answer(session: &PermissionSession, decision: Decision) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(request) = session.pending() {
                    assert!(session
                        .resolve(request.task_id + 1, &request.id, decision)
                        .is_err());
                    assert!(session.resolve(request.task_id, "stale", decision).is_err());
                    session
                        .resolve(request.task_id, &request.id, decision)
                        .unwrap();
                    assert!(session
                        .resolve(request.task_id, &request.id, decision)
                        .is_err());
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn remembered_scope_is_exact_project_local_and_revocable() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"scripts":{"test":"node test.js"}}"#,
        )
        .unwrap();
        let (store, project, task) = setup(root.path());
        let session = PermissionSession::new(store.clone(), project).unwrap();
        let scope = scope(
            "run_command",
            &json!({"program":"npm","args":["test"]}),
            root.path(),
            &session.git_network,
        )
        .unwrap();
        let sink = Sink::default();
        let record = Recorder {
            task_id: task,
            store: &store,
            sink: &sink,
        };
        let cancel = AtomicBool::new(false);
        let (approved, _) = tokio::join!(
            session.authorize(scope.clone(), "test", &cancel, None, &record),
            answer(&session, Decision::Remember)
        );
        approved.unwrap();
        let next = PermissionSession::new(store.clone(), project).unwrap();
        next.authorize(scope.clone(), "test", &cancel, None, &record)
            .await
            .unwrap();
        assert!(next.pending().is_none());
        assert_eq!(execution::load(&store, project).unwrap().grants.len(), 1);
        let changed = super::scope(
            "run_command",
            &json!({"program":"npm","args":["test","--","other"]}),
            root.path(),
            &session.git_network,
        )
        .unwrap();
        assert_ne!(scope.id, changed.id);
        std::fs::write(
            root.path().join("package.json"),
            r#"{"scripts":{"test":"node changed.js"}}"#,
        )
        .unwrap();
        assert_ne!(
            scope.id,
            super::scope(
                "run_command",
                &json!({"program":"npm","args":["test"]}),
                root.path(),
                &session.git_network
            )
            .unwrap()
            .id
        );
        let another = tempfile::tempdir().unwrap();
        let other = store::project::insert_project(
            &store.conn(),
            another.path().to_str().unwrap(),
            "other",
        )
        .unwrap();
        assert!(execution::load(&store, other).unwrap().grants.is_empty());
        execution::revoke(&store, project, &scope.id).unwrap();
        let (denied, _) = tokio::join!(
            next.authorize(scope, "test", &cancel, None, &record),
            answer(&next, Decision::Deny)
        );
        assert!(matches!(denied, Err(XaimError::Cancelled)));
    }
    #[tokio::test]
    async fn once_does_not_persist_and_cancelled_or_dropped_requests_cannot_be_approved() {
        let root = tempfile::tempdir().unwrap();
        let (store, project, task) = setup(root.path());
        let session = PermissionSession::new(store.clone(), project).unwrap();
        let scope = scope(
            "run_command",
            &json!({"program":"git","args":["--version"]}),
            root.path(),
            &session.git_network,
        )
        .unwrap();
        let sink = Sink::default();
        let record = Recorder {
            task_id: task,
            store: &store,
            sink: &sink,
        };
        let cancel = AtomicBool::new(false);
        let (approved, _) = tokio::join!(
            session.authorize(scope.clone(), "test", &cancel, None, &record),
            answer(&session, Decision::Once)
        );
        approved.unwrap();
        assert!(execution::load(&store, project).unwrap().grants.is_empty());
        session
            .authorize(scope.clone(), "test", &cancel, None, &record)
            .await
            .unwrap();
        let next = PermissionSession::new(store.clone(), project).unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(50),
            next.authorize(scope.clone(), "test", &cancel, None, &record),
        )
        .await;
        assert!(result.is_err());
        assert!(next.pending().is_none());
        cancel.store(true, Ordering::SeqCst);
        assert!(matches!(
            next.authorize(scope.clone(), "test", &cancel, None, &record)
                .await,
            Err(XaimError::Cancelled)
        ));
        assert!(next.resolve(task, "stale", Decision::Remember).is_err());
        cancel.store(false, Ordering::SeqCst);
        execution::configure(&store, project, ExecutionMode::Disabled, Default::default()).unwrap();
        assert!(matches!(
            session
                .authorize(scope, "test", &cancel, None, &record)
                .await,
            Err(XaimError::Blocked(_))
        ));
    }
}
