//! Bounded native tool execution in a private snapshot, followed by user review.
//! Confirmation and undo affect only task files and preserve Git history.

#[cfg(test)]
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::config::model::{AppConfig, ModelConfig};
use crate::errors::{Result, XaimError};
use crate::events::{self, XaimEvent};
use crate::index;
use crate::model::provider::{CompletionRequest, Message, ModelProvider};
use crate::model::router::{pick_model, RouterConfig, TaskKind};
use crate::repair::r#loop::EventSink;
use crate::search::context::{build_context, Context};
use crate::search::grep::{self, GrepOptions};
use crate::search::rank::{rank_results, RankedFile};
use crate::search::symbol::search_symbols_fuzzy;
use crate::store::{self, Store};
use crate::tools::patch::{self, PatchCandidate};
use crate::tools::r#trait::ToolContext;
use crate::types::TaskStatus;
use crate::verify::detect::detect_verify_config;
use crate::verify::runner::{self, VerifyKind, VerifyResult, DEFAULT_TIMEOUT_SEC};

use super::planner::{self, Intent, Plan};
use super::tool_loop::{self, LoopConfig, Recorder};
use super::workspace::{ChangeSet, TaskWorkspace};

/// 编排配置：模型连接与路由、每步超时与候选策略。
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// 模型提供方连接配置（生产构造 provider 用）。
    pub model: ModelConfig,
    /// 任务 → 模型路由配置。
    pub router: RouterConfig,
    /// 单个步骤的超时秒数。
    pub step_timeout_sec: u64,
    /// 兼容补丁路径的候选数量；原生工具循环不使用候选。
    pub candidate_count: usize,
    /// 进入上下文的相关文件数上限。
    pub context_top_files: usize,
    /// 上下文 token 预算。
    pub context_budget_tokens: usize,
    /// 工具上下文透传：危险操作是否需用户确认。
    pub require_confirm: bool,
    /// 工具上下文透传：exec 白名单外额外放行的命令。
    pub allowed_commands: Vec<String>,
    pub tool_loop: LoopConfig,
    pub task_timeout_sec: u64,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            model: ModelConfig::default(),
            router: RouterConfig::default(),
            step_timeout_sec: 600,
            candidate_count: 1,
            context_top_files: 8,
            context_budget_tokens: 8192,
            require_confirm: false,
            allowed_commands: Vec::new(),
            tool_loop: LoopConfig::default(),
            task_timeout_sec: 1800,
        }
    }
}

/// 任务编排器：驱动一个任务从解析意图到等待确认的完整流程。
///
/// 事件出口经 [`EventSink`] 抽象（生产转发 Tauri 事件通道，测试用内存收集器），
/// 取消经 [`AtomicBool`] 标记在步骤边界生效。同一实例同一时间驱动一个任务。
///
/// 注意：不在字段里保存 `AppHandle`（生产构造把它交给 [`ChannelSink`]）——
/// 在结构体布局中引用 AppHandle 会把 tao/wry 窗口运行时整段拉进链接产物，
/// 导致纯后端的单元测试二进制在启动期解析窗口层导入失败。
#[derive(Clone)]
pub struct Orchestrator {
    store: Arc<Store>,
    config: OrchestratorConfig,
    cancel_flag: Arc<AtomicBool>,
    /// 当前驱动任务的任务 id（0 表示尚未创建任务记录），供命令层取消时核对。
    current_task_id: Arc<AtomicI64>,
    sink: Arc<dyn EventSink>,
    provider: Arc<dyn ModelProvider>,
}

impl Orchestrator {
    /// 生产构造：从模型配置创建 provider，事件经 Tauri 通道推送。
    pub fn new(app: AppHandle, store: Arc<Store>, config: OrchestratorConfig) -> Result<Self> {
        let provider: Arc<dyn ModelProvider> =
            Arc::from(crate::model::create_provider(&config.model)?);
        Ok(Self {
            store,
            config,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            current_task_id: Arc::new(AtomicI64::new(0)),
            sink: Arc::new(ChannelSink(Some(app))),
            provider,
        })
    }

    /// 测试构造：事件出口与 provider 均由调用方注入，不依赖 Tauri 运行时。
    pub fn with_sink(
        store: Arc<Store>,
        config: OrchestratorConfig,
        sink: Arc<dyn EventSink>,
        provider: Arc<dyn ModelProvider>,
    ) -> Self {
        Self {
            store,
            config,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            current_task_id: Arc::new(AtomicI64::new(0)),
            sink,
            provider,
        }
    }

    /// 请求取消当前正在运行的任务：流程在下一个步骤边界停止并转为
    /// Cancelled；已结束（含已到达等待确认）的任务不受影响。
    pub async fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    /// 当前驱动任务的任务 id；尚未创建任务记录（流程刚开始）时返回 `None`。
    pub fn current_task_id(&self) -> Option<i64> {
        let id = self.current_task_id.load(Ordering::SeqCst);
        (id != 0).then_some(id)
    }

    /// 在隔离副本中执行任务，最终差异进入 WaitingConfirm，返回任务 id。
    /// 原生工具协议是默认路径；文本补丁路径仅用于兼容 provider。
    ///
    /// 错误不崩溃：任何步骤失败（含取消与超时）都会把任务转入终态并推送
    /// 事件，再以原错误返回。
    pub async fn run_task(&self, project_id: i64, prompt: String) -> Result<i64> {
        // 上一次 cancel 残留的标记不应影响新任务。
        self.cancel_flag.store(false, Ordering::SeqCst);
        let started = Instant::now();

        let project = {
            let conn = self.store.conn();
            store::project::get_project(&conn, project_id)?
        }
        .ok_or_else(|| XaimError::Config(format!("项目 #{project_id} 不存在")))?;
        let root = PathBuf::from(&project.path);
        if !root.is_dir() {
            return Err(XaimError::Config(format!(
                "项目目录不存在: {}",
                project.path
            )));
        }

        // 1. 创建任务记录（Idle），随后第一步转入 Parsing。
        let task_id = {
            let conn = self.store.conn();
            store::task::insert_task(&conn, project_id, &prompt)?
        };
        self.current_task_id.store(task_id, Ordering::SeqCst);
        let ctx = ToolContext {
            project_root: root.clone(),
            store: self.store.clone(),
            require_confirm: self.config.require_confirm,
            allowed_commands: self.config.allowed_commands.clone(),
        };

        let deadline = started + Duration::from_secs(self.config.task_timeout_sec);
        let outcome = self
            .drive(task_id, project_id, &prompt, &root, &ctx, deadline)
            .await;
        match outcome {
            Ok(()) => {
                tracing::info!(
                    task_id,
                    elapsed_ms = started.elapsed().as_millis() as i64,
                    "任务流程到达等待确认"
                );
                Ok(task_id)
            }
            Err(error) => {
                self.finish_abnormally(task_id, &error);
                Err(error)
            }
        }
    }

    /// Apply only the reviewed task changes. Git commits require separate authorization.
    ///
    /// 状态机严格：仅接受 WaitingConfirm（-> Applied -> Done）。
    pub async fn confirm_task(&self, task_id: i64) -> Result<()> {
        let (task, patches) = {
            let conn = self.store.conn();
            let task = store::task::get_task(&conn, task_id)?
                .ok_or_else(|| XaimError::Config(format!("任务 #{task_id} 不存在")))?;
            let patches = store::patch::list_patches_by_task(&conn, task_id)?;
            (task, patches)
        };
        if task.status != TaskStatus::WaitingConfirm {
            return Err(XaimError::Config(format!(
                "任务 #{task_id} 状态为 {:?}，不在等待确认阶段",
                task.status
            )));
        }
        if patches.is_empty() {
            return Err(XaimError::Config(format!(
                "任务 #{task_id} 没有可应用的补丁"
            )));
        }
        let root = PathBuf::from({
            let conn = self.store.conn();
            store::project::get_project(&conn, task.project_id)?
                .ok_or_else(|| XaimError::Config(format!("项目 #{} 不存在", task.project_id)))?
                .path
        });

        let changes = {
            let conn = self.store.conn();
            let json = store::cache::get_cache(&conn, &format!("task:{task_id}:changes"))?
                .ok_or_else(|| {
                    XaimError::PatchConflict("此旧任务缺少文件基线，请重新运行后确认".into())
                })?;
            serde_json::from_str::<ChangeSet>(&json)?
        };

        // WaitingConfirm -> Applied
        let mut status = task.status;
        self.emit_step(task_id, "应用补丁", "running")?;
        if let Err(error) = changes.apply(&root, false) {
            self.emit_step(task_id, "应用补丁", "failed")?;
            return Err(error);
        }
        self.transition(task_id, &mut status, TaskStatus::Applied)?;

        // Applied -> Done（finish_task 同时写入 finished_at 与 duration_ms）
        if !status.can_transition_to(&TaskStatus::Done) {
            return Err(XaimError::Unknown(format!(
                "非法状态转移 {status:?} -> Done"
            )));
        }
        {
            let conn = self.store.conn();
            for patch in &patches {
                store::patch::mark_applied(&conn, patch.id)?;
            }
            store::task::finish_task(&conn, task_id, TaskStatus::Done)?;
        }
        self.emit_step(task_id, "应用补丁", "done")?;
        self.sink.emit(XaimEvent::Done {
            task_id,
            status: status_label(&TaskStatus::Done).to_string(),
        })?;
        Ok(())
    }

    /// 用户拒绝候选补丁：任务从 WaitingConfirm 转入 Cancelled（通配中止）。
    ///
    /// 等待确认阶段工作区未被改动，无需恢复；补丁保持未应用状态留档。
    /// 仅接受 WaitingConfirm。
    pub async fn reject_task(&self, task_id: i64) -> Result<()> {
        let status = {
            let conn = self.store.conn();
            store::task::get_task(&conn, task_id)?
                .ok_or_else(|| XaimError::Config(format!("任务 #{task_id} 不存在")))?
                .status
        };
        if status != TaskStatus::WaitingConfirm {
            return Err(XaimError::Config(format!(
                "任务 #{task_id} 状态为 {status:?}，不在等待确认阶段"
            )));
        }
        {
            let conn = self.store.conn();
            store::task::finish_task(&conn, task_id, TaskStatus::Cancelled)?;
        }
        tracing::info!(task_id, "用户拒绝补丁，任务取消");
        self.sink.emit(XaimEvent::Done {
            task_id,
            status: status_label(&TaskStatus::Cancelled).to_string(),
        })?;
        Ok(())
    }

    /// Undo only task files whose contents still match the applied version.
    /// Done -> RolledBack is a user-requested administrative operation.
    pub async fn rollback_task(&self, task_id: i64) -> Result<()> {
        let (status, root) = {
            let conn = self.store.conn();
            let task = store::task::get_task(&conn, task_id)?
                .ok_or_else(|| XaimError::Config(format!("任务 #{task_id} 不存在")))?;
            let root = store::project::get_project(&conn, task.project_id)?
                .ok_or_else(|| XaimError::Config(format!("项目 #{} 不存在", task.project_id)))?
                .path;
            (task.status, PathBuf::from(root))
        };
        if !matches!(status, TaskStatus::Applied | TaskStatus::Done) {
            return Err(XaimError::Config(format!(
                "任务 #{task_id} 状态为 {status:?}，没有已应用的补丁可回滚"
            )));
        }

        let changes = {
            let conn = self.store.conn();
            let json = store::cache::get_cache(&conn, &format!("task:{task_id}:changes"))?
                .ok_or_else(|| {
                    XaimError::PatchConflict("此旧任务缺少文件基线，无法安全撤销".into())
                })?;
            serde_json::from_str::<ChangeSet>(&json)?
        };
        changes.apply(&root, true)?;
        {
            let conn = self.store.conn();
            conn.execute(
                "UPDATE patches SET applied = 0 WHERE task_id = ?1",
                [task_id],
            )?;
            store::task::finish_task(&conn, task_id, TaskStatus::RolledBack)?;
        }
        tracing::info!(task_id, "任务补丁已回滚");
        self.sink.emit(XaimEvent::Done {
            task_id,
            status: status_label(&TaskStatus::RolledBack).to_string(),
        })?;
        Ok(())
    }

    /// 主流程：逐步推进状态机，任一步失败即中断（由 [`Self::run_task`] 收尾）。
    async fn drive(
        &self,
        task_id: i64,
        project_id: i64,
        prompt: &str,
        root: &Path,
        ctx: &ToolContext,
        deadline: Instant,
    ) -> Result<()> {
        if self.provider.supports_tools() {
            return self
                .drive_tools(task_id, project_id, prompt, root, ctx, deadline)
                .await;
        }
        // Compatibility path also operates entirely in a private snapshot.
        let workspace = TaskWorkspace::snapshot(root)?;
        let isolated_ctx = ToolContext {
            project_root: workspace.root().into(),
            ..ctx.clone()
        };
        let root = workspace.root();
        let ctx = &isolated_ctx;
        let mut status = TaskStatus::Idle;
        let provider = self.provider.as_ref();

        // 2. 解析意图（Parsing）
        let model = self.routed_model(TaskKind::Intent);
        let lang = index::lang::detect_project_language(root);
        let intent: Intent = self
            .run_step(
                task_id,
                &mut status,
                TaskStatus::Parsing,
                "解析意图",
                planner::parse_intent_with_model(prompt, &lang, &model, provider),
            )
            .await?;
        {
            let intent_json = serde_json::to_string(&intent)?;
            let conn = self.store.conn();
            store::task::update_task_intent(&conn, task_id, &intent_json)?;
            store::task::insert_step(&conn, task_id, "intent", &intent_json)?;
        }

        // 3. 检查索引（Indexing）：空索引全量建，否则按 mtime 增量更新。
        let index_store = self.store.clone();
        let index_root = root.to_path_buf();
        self.run_step(
            task_id,
            &mut status,
            TaskStatus::Indexing,
            "检查索引",
            async move { ensure_indexed(index_store, project_id, index_root).await },
        )
        .await?;

        // 4. 检索相关文件（Retrieving）
        let retrieve_store = self.store.clone();
        let retrieve_task = prompt.to_string();
        let retrieve_target = intent.target.clone();
        let retrieve_root = root.to_path_buf();
        let top_files = self.config.context_top_files;
        let budget = self.config.context_budget_tokens;
        let context: Context = self
            .run_step(
                task_id,
                &mut status,
                TaskStatus::Retrieving,
                "检索相关文件",
                async move {
                    tokio::task::spawn_blocking(move || {
                        retrieve_context(
                            retrieve_store,
                            project_id,
                            &retrieve_task,
                            &retrieve_target,
                            &retrieve_root,
                            top_files,
                            budget,
                        )
                    })
                    .await
                    .map_err(|error| XaimError::Unknown(format!("检索线程异常退出: {error}")))?
                },
            )
            .await?;

        // 5. 生成计划（Planning）
        let model = self.routed_model(TaskKind::Generate);
        let plan: Plan = self
            .run_step(
                task_id,
                &mut status,
                TaskStatus::Planning,
                "生成计划",
                planner::generate_plan_with_model(prompt, &context, &model, provider),
            )
            .await?;
        {
            let plan_json = serde_json::to_string(&plan)?;
            let conn = self.store.conn();
            store::task::update_task_plan(&conn, task_id, &plan_json)?;
            store::task::insert_step(&conn, task_id, "plan", &plan_json)?;
        }

        // 6. 生成候选 Patch（Generating）
        let candidate_count = self.config.candidate_count;
        let candidates = self
            .run_step(
                task_id,
                &mut status,
                TaskStatus::Generating,
                "生成候选",
                planner::generate_candidates_in_workspace(
                    prompt,
                    &plan,
                    &context,
                    &model,
                    provider,
                    candidate_count,
                    root,
                ),
            )
            .await?;
        tracing::info!(task_id, count = candidates.len(), "候选补丁已生成");

        // 7a. 补丁预检（ApplyingPatch）：dry-run 过滤无法应用的候选。
        let applicable = self
            .run_step(
                task_id,
                &mut status,
                TaskStatus::ApplyingPatch,
                "检查补丁",
                async {
                    let mut applicable = Vec::new();
                    for candidate in &candidates {
                        match patch::validate_diff(&candidate.diff, &ctx.project_root) {
                            Ok(()) => applicable.push(candidate.clone()),
                            Err(error) => tracing::warn!(
                                file_path = %candidate.file_path,
                                error = %error,
                                "候选补丁预检失败，已排除"
                            ),
                        }
                    }
                    if applicable.is_empty() {
                        return Err(XaimError::PatchConflict("所有候选补丁都无法应用".into()));
                    }
                    Ok(applicable)
                },
            )
            .await?;

        // 7b. 评估候选（Verifying）：应用 -> 验证 -> 评分 -> 回滚。
        let review_model = self.routed_model(TaskKind::Summarize);
        let best = self
            .run_step(
                task_id,
                &mut status,
                TaskStatus::Verifying,
                "评估候选",
                evaluate_candidates_with_model(applicable, ctx, provider, &review_model),
            )
            .await?;
        patch::apply_diff(&best.diff, root)?;
        self.save_changes(task_id, &workspace.changes()?)?;

        // 8. 记录最优补丁并等待确认（WaitingConfirm）
        self.run_step(
            task_id,
            &mut status,
            TaskStatus::WaitingConfirm,
            "等待确认",
            async {
                let patch_id = {
                    let conn = self.store.conn();
                    store::patch::insert_patch(
                        &conn,
                        task_id,
                        &best.file_path,
                        &best.diff,
                        best.score,
                    )?
                };
                self.emit_patch(task_id, &best)?;
                tracing::info!(
                    task_id,
                    patch_id,
                    file = %best.file_path,
                    score = ?best.score,
                    "最优候选已记录，等待用户确认"
                );
                Ok(())
            },
        )
        .await?;
        Ok(())
    }

    fn save_changes(&self, task_id: i64, changes: &ChangeSet) -> Result<()> {
        let conn = self.store.conn();
        store::cache::set_cache(
            &conn,
            &format!("task:{task_id}:changes"),
            &serde_json::to_string(changes)?,
            None,
        )
    }

    fn preserve_partial_changes(&self, task_id: i64, workspace: &TaskWorkspace) -> Result<()> {
        let record = Recorder {
            task_id,
            store: &self.store,
            sink: self.sink.as_ref(),
        };
        let snapshot =
            store::cache::get_cache(&self.store.conn(), &format!("task:{task_id}:snapshot"))?;
        if let Some(saved) = snapshot {
            let mut snapshot: super::task_context::TaskSnapshot = serde_json::from_str(&saved)?;
            snapshot.phase = "partial".into();
            snapshot.ready_for_review = false;
            snapshot.save(workspace, &record)?;
            if store::cache::get_cache(&self.store.conn(), &format!("task:{task_id}:report"))?
                .is_none()
            {
                let report =
                    snapshot.save_report(workspace, &record, "执行中断，已保存阶段性成果")?;
                record.event(XaimEvent::Message {
                    task_id,
                    content: report,
                })?;
            }
        }
        let changes = workspace.changes()?;
        if changes.files.is_empty() {
            return Ok(());
        }
        let candidates: Vec<PatchCandidate> = changes
            .files
            .iter()
            .map(|file| PatchCandidate {
                file_path: file.path.clone(),
                diff: ChangeSet {
                    files: vec![file.clone()],
                }
                .diff(),
                score: None,
            })
            .collect();
        {
            let conn = self.store.conn();
            let tx = conn.unchecked_transaction()?;
            store::cache::set_cache(
                &tx,
                &format!("task:{task_id}:partial_changes"),
                &serde_json::to_string(&changes)?,
                None,
            )?;
            for candidate in &candidates {
                store::patch::insert_patch(
                    &tx,
                    task_id,
                    &candidate.file_path,
                    &candidate.diff,
                    None,
                )?;
            }
            tx.commit()?;
        }
        let record = Recorder {
            task_id,
            store: &self.store,
            sink: self.sink.as_ref(),
        };
        record.event(XaimEvent::Message {
            task_id,
            content: "已保留阶段性代码变更：任务完成情况尚未确认，检查结果见验证详情，仅供审阅，未应用到项目。"
                .into(),
        })?;
        for candidate in candidates {
            self.emit_patch(task_id, &candidate)?;
        }
        Ok(())
    }

    async fn drive_tools(
        &self,
        task_id: i64,
        _project_id: i64,
        prompt: &str,
        root: &Path,
        ctx: &ToolContext,
        deadline: Instant,
    ) -> Result<()> {
        let mut status = TaskStatus::Idle;
        self.run_step(
            task_id,
            &mut status,
            TaskStatus::Parsing,
            "接收任务",
            async { Ok(()) },
        )
        .await?;
        let workspace = self
            .run_step(
                task_id,
                &mut status,
                TaskStatus::Indexing,
                "保存工作区基线",
                async { TaskWorkspace::snapshot(root) },
            )
            .await?;
        self.run_step(
            task_id,
            &mut status,
            TaskStatus::Retrieving,
            "准备文件检索工具",
            async { Ok(()) },
        )
        .await?;
        self.run_step(
            task_id,
            &mut status,
            TaskStatus::Planning,
            "准备受控执行",
            async { Ok(()) },
        )
        .await?;
        let isolated_ctx = ToolContext {
            project_root: workspace.root().into(),
            ..ctx.clone()
        };
        let model = self.routed_model(TaskKind::Generate);
        let mut loop_config = self.config.tool_loop.clone();
        loop_config.verification_source = Some(root.into());
        loop_config.deadline = Some(deadline);
        let result = self
            .run_step(
                task_id,
                &mut status,
                TaskStatus::Generating,
                "执行工具循环",
                tool_loop::cancellable_until(
                    &self.cancel_flag,
                    deadline,
                    tool_loop::run(
                        prompt,
                        &model,
                        self.provider.as_ref(),
                        &workspace,
                        &isolated_ctx,
                        &loop_config,
                        self.cancel_flag.clone(),
                        Recorder {
                            task_id,
                            store: &self.store,
                            sink: self.sink.as_ref(),
                        },
                    ),
                ),
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let _ = (Recorder { task_id, store: &self.store, sink: self.sink.as_ref() })
                    .diagnostic("taskStop", serde_json::json!({"reason":match &error {
                        XaimError::Timeout(_) => "task_deadline", XaimError::Cancelled => "cancel_requested",
                        XaimError::Verify(_) => "verification", _ => "execution_error",
                    }}));
                if let Err(save_error) = self.preserve_partial_changes(task_id, &workspace) {
                    tracing::error!(task_id, error = %save_error, "阶段性变更保存失败");
                    let _ = (Recorder {
                        task_id,
                        store: &self.store,
                        sink: self.sink.as_ref(),
                    })
                    .diagnostic(
                        "partialChangesFailed",
                        serde_json::json!({"error":save_error.to_string()}),
                    );
                }
                return Err(error);
            }
        };
        self.run_step(
            task_id,
            &mut status,
            TaskStatus::ApplyingPatch,
            "汇总代码变更",
            async { self.save_changes(task_id, &result.changes) },
        )
        .await?;
        self.run_step(
            task_id,
            &mut status,
            TaskStatus::Verifying,
            "记录最终验证状态",
            async { Ok(()) },
        )
        .await?;
        self.run_step(
            task_id,
            &mut status,
            TaskStatus::WaitingConfirm,
            "等待确认",
            async {
                for file in &result.changes.files {
                    let candidate = PatchCandidate {
                        file_path: file.path.clone(),
                        diff: ChangeSet {
                            files: vec![file.clone()],
                        }
                        .diff(),
                        score: None,
                    };
                    if candidate.diff.is_empty() {
                        return Err(XaimError::PatchConflict("变更无法生成文本差异".into()));
                    }
                    {
                        let conn = self.store.conn();
                        store::patch::insert_patch(
                            &conn,
                            task_id,
                            &candidate.file_path,
                            &candidate.diff,
                            None,
                        )?;
                    }
                    self.emit_patch(task_id, &candidate)?;
                }
                Ok(())
            },
        )
        .await?;
        Ok(())
    }

    /// 统一步骤骨架：取消检查 -> 严格状态转移 -> Step 事件 -> 超时包裹 -> 完成事件。
    async fn run_step<F, T>(
        &self,
        task_id: i64,
        status: &mut TaskStatus,
        next: TaskStatus,
        name: &str,
        step: F,
    ) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        if self.cancel_flag.load(Ordering::SeqCst) {
            tracing::warn!(task_id, step = name, "任务已取消");
            return Err(XaimError::Cancelled);
        }
        self.transition(task_id, status, next)?;
        self.emit_step(task_id, name, "running")?;

        let timeout_sec = if next == TaskStatus::Generating && self.provider.supports_tools() {
            self.config.task_timeout_sec
        } else {
            self.config.step_timeout_sec
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(timeout_sec),
            tool_loop::cancellable(&self.cancel_flag, self.config.task_timeout_sec, step),
        )
        .await;
        match outcome {
            Ok(Ok(value)) => {
                self.emit_step(task_id, name, "done")?;
                Ok(value)
            }
            Ok(Err(error)) => {
                tracing::warn!(task_id, step = name, error = %error, "步骤失败");
                if let Err(emit_error) = self.emit_step(
                    task_id,
                    name,
                    if matches!(error, XaimError::Cancelled) {
                        "cancelled"
                    } else {
                        "failed"
                    },
                ) {
                    tracing::error!(task_id, error = %emit_error, "步骤终态记录失败");
                }
                Err(error)
            }
            Err(_elapsed) => {
                let error =
                    XaimError::Timeout(format!("步骤「{name}」超过 {} 秒未完成", timeout_sec));
                tracing::warn!(task_id, step = name, error = %error, "步骤超时");
                if let Err(emit_error) = self.emit_step(task_id, name, "failed") {
                    tracing::error!(task_id, error = %emit_error, "步骤终态记录失败");
                }
                Err(error)
            }
        }
    }

    /// 状态机严格校验的转移：校验通过才落库并更新本地跟踪。
    fn transition(&self, task_id: i64, status: &mut TaskStatus, next: TaskStatus) -> Result<()> {
        if !status.can_transition_to(&next) {
            return Err(XaimError::Unknown(format!(
                "非法状态转移 {status:?} -> {next:?}"
            )));
        }
        {
            let conn = self.store.conn();
            store::task::update_task_status(&conn, task_id, next)?;
        }
        tracing::debug!(task_id, from = ?status, to = ?next, "状态转移");
        *status = next;
        Ok(())
    }

    fn emit_step(&self, task_id: i64, name: &str, step_status: &str) -> Result<()> {
        {
            let conn = self.store.conn();
            store::task::insert_step(&conn, task_id, name, step_status)?;
        }
        self.sink.emit(XaimEvent::Step {
            task_id,
            name: name.to_string(),
            status: step_status.to_string(),
        })
    }

    fn emit_patch(&self, task_id: i64, candidate: &PatchCandidate) -> Result<()> {
        self.sink.emit(XaimEvent::Patch {
            task_id,
            file: candidate.file_path.clone(),
            diff: candidate.diff.clone(),
        })
    }

    /// 按路由配置解析某类任务使用的模型名。
    fn routed_model(&self, kind: TaskKind) -> String {
        let config = AppConfig {
            data_dir: String::new(),
            model: self.config.model.clone(),
            router: self.config.router.clone(),
        };
        pick_model(kind, &config).name().to_string()
    }

    /// 统一异常出口：任务转入终态（取消/失败）并推送事件，不掩盖原始错误。
    fn finish_abnormally(&self, task_id: i64, error: &XaimError) {
        let cancelled = matches!(error, XaimError::Cancelled);
        let terminal = if cancelled {
            TaskStatus::Cancelled
        } else {
            TaskStatus::Failed
        };

        let current = {
            let conn = self.store.conn();
            store::task::get_task(&conn, task_id)
                .ok()
                .flatten()
                .map(|task| task.status)
        };
        if let Some(current) = current {
            if current.can_transition_to(&terminal) {
                if let Err(store_error) = {
                    let conn = self.store.conn();
                    store::task::finish_task(&conn, task_id, terminal)
                } {
                    tracing::error!(task_id, error = %store_error, "终态写入失败");
                }
            } else {
                tracing::error!(
                    task_id,
                    from = ?current,
                    terminal = ?terminal,
                    "无法转移至终态，保持现状"
                );
            }
        }

        if !cancelled {
            if let Err(emit_error) = (Recorder {
                task_id,
                store: &self.store,
                sink: self.sink.as_ref(),
            })
            .event(XaimEvent::Error {
                task_id,
                message: error.to_string(),
            }) {
                tracing::error!(task_id, error = %emit_error, "错误事件推送失败");
            }
        }
        if let Err(emit_error) = (Recorder {
            task_id,
            store: &self.store,
            sink: self.sink.as_ref(),
        })
        .event(XaimEvent::Done {
            task_id,
            status: status_label(&terminal).to_string(),
        }) {
            tracing::error!(task_id, error = %emit_error, "完成事件推送失败");
        }
    }
}

/// 生产事件出口：事件经 Tauri 全局通道推送；无句柄时静默丢弃。
struct ChannelSink(Option<AppHandle>);

impl EventSink for ChannelSink {
    fn emit(&self, event: XaimEvent) -> Result<()> {
        match &self.0 {
            Some(app) => events::emit(app, event),
            None => Ok(()),
        }
    }
}

/// 终态的事件标签（与 TaskStatus 的 camelCase 序列化一致）。
fn status_label(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Done => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::RolledBack => "rolledBack",
        _ => "unknown",
    }
}

/// 索引检查：索引为空则全量扫描，否则按 mtime 增量更新
/// （磁盘仍在的更新索引，已删除的移除记录）。
async fn ensure_indexed(store: Arc<Store>, project_id: i64, root: PathBuf) -> Result<()> {
    let indexed = {
        let conn = store.conn();
        store::file::list_files(&conn, project_id)?
    };
    if indexed.is_empty() {
        tracing::info!(project_id, "索引为空，执行全量索引");
        return index::store::index_project_async(Store::clone(&store), project_id, root).await;
    }
    let stale = index::store::get_stale_files(&store, project_id)?;
    tracing::info!(project_id, stale = stale.len(), "增量更新索引");
    for path in stale {
        if root.join(&path).exists() {
            index::store::update_file_index(&store, project_id, &path)?;
        } else {
            index::store::remove_file_index(&store, project_id, &path)?;
        }
    }
    Ok(())
}

/// 检索并组装上下文：任务关键词检索内容与符号，融合文件元数据重排，
/// 取 top N 交给 [`build_context`]。grep / 符号检索失败不阻断流程
/// （上下文退化为索引文件清单），文件清单读取失败才算步骤失败。
fn retrieve_context(
    store: Arc<Store>,
    project_id: i64,
    task: &str,
    target: &str,
    root: &Path,
    top_files: usize,
    budget_tokens: usize,
) -> Result<Context> {
    let query = target.trim();
    let query = if query.is_empty() { task } else { query };

    let grep_matches = grep::search(root, query, GrepOptions::default()).unwrap_or_else(|error| {
        tracing::warn!(error = %error, query, "内容检索失败，跳过 grep 结果");
        Vec::new()
    });
    let symbols = search_symbols_fuzzy(&store, project_id, query).unwrap_or_else(|error| {
        tracing::warn!(error = %error, query, "符号检索失败，跳过符号结果");
        Vec::new()
    });
    let files = {
        let conn = store.conn();
        store::file::list_files(&conn, project_id)?
    };

    let ranked = rank_results(task, grep_matches, symbols, files);
    let top: Vec<RankedFile> = ranked.into_iter().take(top_files).collect();
    tracing::info!(query, files = top.len(), "检索完成");
    build_context(task, top, root, budget_tokens)
}

/// 置信度评审提示词：轻量判定任务，产出 `{"confidence": 0..1}`。
const CONFIDENCE_SYSTEM: &str = "你是代码评审员。阅读给定的 unified diff，\
评估它作为一次代码修改的正确性与质量置信度。\
只输出一个 JSON 对象：{\"confidence\": <0.0 到 1.0 之间的小数>}，不要输出其他内容。";

/// 置信度评审失败时的中性值。
const NEUTRAL_CONFIDENCE: f64 = 0.5;

/// 评估全部候选并返回得分最高者（`score` 字段已填入）。
///
/// 每个候选独立评估：备份目标文件 -> 应用补丁 -> 运行项目验证命令 ->
/// 模型置信度评审 -> 按公式评分 -> 回滚工作区。任何一步失败只作废该候选
/// （工作区先恢复原状），其余候选不受影响；全部失败时返回
/// [`XaimError::Verify`]。并列最高分时取先评估者。
pub async fn evaluate_candidates(
    candidates: Vec<PatchCandidate>,
    ctx: &ToolContext,
    provider: &dyn ModelProvider,
) -> Result<PatchCandidate> {
    let model = pick_model(TaskKind::Summarize, &AppConfig::default());
    evaluate_candidates_with_model(candidates, ctx, provider, model.name()).await
}

async fn evaluate_candidates_with_model(
    candidates: Vec<PatchCandidate>,
    ctx: &ToolContext,
    provider: &dyn ModelProvider,
    model: &str,
) -> Result<PatchCandidate> {
    let max_diff_size = candidates.iter().map(|c| c.diff.len()).max().unwrap_or(0);
    let mut best: Option<PatchCandidate> = None;
    let mut best_score = f64::NEG_INFINITY;
    let mut last_error = String::new();

    for candidate in candidates {
        match evaluate_one(&candidate, ctx, provider, max_diff_size, model).await {
            Ok(score) => {
                tracing::info!(file_path = %candidate.file_path, score, "候选评分完成");
                if score > best_score {
                    best_score = score;
                    best = Some(PatchCandidate {
                        score: Some(score),
                        ..candidate
                    });
                }
            }
            Err(error) => {
                tracing::warn!(file_path = %candidate.file_path, error = %error, "候选评估失败，已跳过");
                last_error = error.to_string();
            }
        }
    }

    best.ok_or_else(|| XaimError::Verify(format!("所有候选评估均失败。最后一次原因：{last_error}")))
}

/// 评估单个候选：备份 -> 应用 -> 验证与评分 -> 回滚。
/// 应用成功后无论评估成败，都先恢复工作区原状再返回。
async fn evaluate_one(
    candidate: &PatchCandidate,
    ctx: &ToolContext,
    provider: &dyn ModelProvider,
    max_diff_size: usize,
    model: &str,
) -> Result<f64> {
    tracing::info!(
        file_path = %candidate.file_path,
        diff_bytes = candidate.diff.len(),
        "开始评估候选"
    );
    let workspace = TaskWorkspace::snapshot(&ctx.project_root)?;
    let isolated_ctx = ToolContext {
        project_root: workspace.root().into(),
        ..ctx.clone()
    };
    let ctx = &isolated_ctx;

    if let Err(error) = patch::apply_diff(&candidate.diff, &ctx.project_root) {
        // apply_diff 两阶段提交：失败时未写盘，无需恢复。
        tracing::warn!(file_path = %candidate.file_path, error = %error, "候选补丁应用失败");
        return Err(error);
    }

    evaluate_applied(candidate, ctx, provider, max_diff_size, model).await
}

/// 对已应用的候选跑验证、置信度评审并评分。
async fn evaluate_applied(
    candidate: &PatchCandidate,
    ctx: &ToolContext,
    provider: &dyn ModelProvider,
    max_diff_size: usize,
    model: &str,
) -> Result<f64> {
    let verifications = run_verifications(ctx).await?;
    let confidence = judge_confidence(provider, candidate, model).await;
    Ok(compute_score(
        candidate.diff.len(),
        max_diff_size,
        &verifications,
        confidence,
    ))
}

/// 运行项目检测到的全部验证命令（类型检查/lint/测试/构建），
/// 逐条记录日志并返回结构化结果。
async fn run_verifications(ctx: &ToolContext) -> Result<Vec<VerifyResult>> {
    let detected = detect_verify_config(&ctx.project_root);
    let commands = [
        detected.type_check.map(|c| (VerifyKind::TypeCheck, c)),
        detected.lint.map(|c| (VerifyKind::Lint, c)),
        detected.test.map(|c| (VerifyKind::Test, c)),
        detected.build.map(|c| (VerifyKind::Build, c)),
    ];

    let mut results = Vec::new();
    for (kind, command) in commands.into_iter().flatten() {
        let result =
            runner::run_verify(kind, &command, &ctx.project_root, DEFAULT_TIMEOUT_SEC).await?;
        tracing::info!(
            kind = result.kind,
            passed = result.passed,
            command = %command,
            "候选验证完成"
        );
        results.push(result);
    }
    Ok(results)
}

/// 让模型为候评选出置信度（0.0-1.0）。
///
/// 评审属轻量判定任务，路由 Fast 档；调用失败或输出无法解析时落为
/// 中性值 [`NEUTRAL_CONFIDENCE`]，不让 0.1 权重的辅助信号拖垮整个评估。
async fn judge_confidence(
    provider: &dyn ModelProvider,
    candidate: &PatchCandidate,
    model: &str,
) -> f64 {
    let request = CompletionRequest::new(
        model,
        vec![
            Message::system(CONFIDENCE_SYSTEM),
            Message::user(format!("unified diff：\n{}", candidate.diff)),
        ],
    );
    let response = match provider.complete(request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(file_path = %candidate.file_path, error = %error, "置信度评审调用失败，使用中性值");
            return NEUTRAL_CONFIDENCE;
        }
    };
    match parse_confidence(&response.content) {
        Some(confidence) => confidence,
        None => {
            tracing::warn!(file_path = %candidate.file_path, "置信度输出无法解析，使用中性值");
            NEUTRAL_CONFIDENCE
        }
    }
}

/// 从模型输出解析置信度：截取 JSON 对象后取 `confidence` 字段，
/// 数字或数字字符串均可，截断到 [0, 1]。
fn parse_confidence(text: &str) -> Option<f64> {
    let json_text = super::planner::extract_json_object(text)?;
    let value: serde_json::Value = serde_json::from_str(json_text).ok()?;
    let raw = value.get("confidence")?;
    let confidence = raw
        .as_f64()
        .or_else(|| raw.as_str().and_then(|s| s.trim().parse::<f64>().ok()))?;
    Some(confidence.clamp(0.0, 1.0))
}

/// 评分公式（纯函数）：
///
/// ```text
/// score = 0.4 * 验证通过率
///       + 0.2 * (1 - diff_size / max_diff_size)
///       + 0.2 * 测试通过
///       + 0.1 * lint 通过
///       + 0.1 * 模型置信度
/// ```
///
/// - 验证通过率 = 通过条数 / 配置总条数（未检测到任何验证命令时为 0）；
/// - 测试/lint 取对应验证类型是否通过（未配置视为未通过）；
/// - diff 越小得分越高（最大者得 0）；`confidence` 截断到 [0, 1]。
///
/// 满分为 1.0。
fn compute_score(
    diff_size: usize,
    max_diff_size: usize,
    verifications: &[VerifyResult],
    confidence: f64,
) -> f64 {
    let verify_ratio = if verifications.is_empty() {
        0.0
    } else {
        verifications.iter().filter(|r| r.passed).count() as f64 / verifications.len() as f64
    };
    let size_component = if max_diff_size == 0 {
        1.0
    } else {
        (1.0 - diff_size as f64 / max_diff_size as f64).max(0.0)
    };
    let kind_passed = |kind: &str| -> f64 {
        match verifications.iter().find(|r| r.kind == kind) {
            Some(r) if r.passed => 1.0,
            _ => 0.0,
        }
    };

    0.4 * verify_ratio
        + 0.2 * size_component
        + 0.2 * kind_passed(VerifyKind::Test.as_str())
        + 0.1 * kind_passed(VerifyKind::Lint.as_str())
        + 0.1 * confidence.clamp(0.0, 1.0)
}

/// 目标文件在评估前的备份。
#[derive(Debug)]
#[cfg(test)]
enum FileBackup {
    /// 文件原本存在，记录原内容。
    Existing(String),
    /// 文件原本不存在（新建文件类补丁），回滚即删除。
    Absent,
}

/// 备份候选目标文件的当前内容。路径校验与 [`patch::apply_diff`] 同源
/// （拒绝绝对路径与越界），保证备份读到的就是补丁将要覆盖的文件。
#[cfg(test)]
fn backup_file(root: &Path, file_path: &str) -> Result<FileBackup> {
    let target = patch::resolve_in_root(root, file_path)?;
    match fs::read_to_string(&target) {
        Ok(content) => Ok(FileBackup::Existing(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileBackup::Absent),
        Err(e) => Err(e.into()),
    }
}

/// 把目标文件恢复到备份时的状态。
#[cfg(test)]
fn restore_file(root: &Path, file_path: &str, backup: &FileBackup) -> Result<()> {
    let target = patch::resolve_in_root(root, file_path)?;
    match backup {
        FileBackup::Existing(content) => {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, content)?;
        }
        FileBackup::Absent => match fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::provider::{CompletionResponse, MockProvider};
    use crate::store::Store;
    use futures::Stream;
    use std::collections::VecDeque;
    use std::fs;
    use std::pin::Pin;
    use std::sync::Mutex;

    const BROKEN_LIB: &str = "pub fn add(a: i32, b: i32) -> i32 { a + }\n";
    const FIXED_LIB: &str = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";
    /// 修复候选：把坏表达式补好。
    const FIX_DIFF: &str = "--- a/src/lib.rs\n\
                            +++ b/src/lib.rs\n\
                            @@ -1 +1 @@\n\
                            -pub fn add(a: i32, b: i32) -> i32 { a + }\n\
                            +pub fn add(a: i32, b: i32) -> i32 { a + b }\n";
    /// 干扰候选：仍是一个坏表达式，验证必然失败。
    const STILL_BROKEN_DIFF: &str = "--- a/src/lib.rs\n\
                                     +++ b/src/lib.rs\n\
                                     @@ -1 +1 @@\n\
                                     -pub fn add(a: i32, b: i32) -> i32 { a + }\n\
                                     +pub fn add(a: i32, b: i32) -> i32 { a + b + }\n";
    /// 上下文对不上的冲突补丁，apply 阶段即失败。
    const CONFLICT_DIFF: &str = "--- a/src/lib.rs\n\
                                 +++ b/src/lib.rs\n\
                                 @@ -9,1 +9,2 @@\n\
                                 完全不存在的上下文行\n\
                                 +oops\n";

    const INTENT_JSON: &str = r#"{"action":"fix_bug","target":"add","description":"修复 add 函数的语法错误","constraints":[],"verify":["test"]}"#;
    const PLAN_JSON: &str = r#"{"steps":[{"action":"modify","file":"src/lib.rs","reason":"修复坏表达式"},{"action":"test","file":"cargo test","reason":"验证修复"}],"files_to_modify":["src/lib.rs"],"risks":[]}"#;
    const CONFIDENCE_JSON: &str = r#"{"confidence": 0.5}"#;

    fn ctx_with(root: &Path) -> ToolContext {
        ToolContext {
            project_root: root.to_path_buf(),
            store: Arc::new(Store::open_in_memory().unwrap()),
            require_confirm: false,
            allowed_commands: vec![],
        }
    }

    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// 最小 cargo 项目：探测器会给出 cargo check/clippy/test/build 四条命令。
    fn cargo_project(root: &Path, lib_body: &str) {
        write_file(root, ".gitignore", "/target\n");
        write_file(
            root,
            "Cargo.toml",
            "[package]\nname = \"xaim-eval-demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_file(root, "src/lib.rs", lib_body);
    }

    fn lib_content(root: &Path) -> String {
        fs::read_to_string(root.join("src/lib.rs")).unwrap()
    }

    fn candidate(diff: &str) -> PatchCandidate {
        PatchCandidate {
            file_path: "src/lib.rs".to_string(),
            diff: diff.to_string(),
            score: None,
        }
    }

    fn verify_result(kind: &str, passed: bool) -> VerifyResult {
        VerifyResult {
            kind: kind.to_string(),
            passed,
            output: String::new(),
            duration_ms: 0,
        }
    }

    /// 建立内存库并登记一个指向 `root` 的项目，返回 (store, project_id)。
    fn setup_project(root: &Path) -> (Arc<Store>, i64) {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let project_id = {
            let conn = store.conn();
            store::project::insert_project(&conn, root.to_str().unwrap(), "demo").unwrap()
        };
        (store, project_id)
    }

    /// 按脚本依次返回响应的 provider：响应耗尽后重复最后一条；
    /// 每次调用先休眠 `delay`，供取消与超时测试控制节奏。
    struct ScriptedProvider {
        responses: Mutex<VecDeque<String>>,
        delay: Duration,
    }

    impl ScriptedProvider {
        fn new(responses: &[&str]) -> Self {
            Self {
                responses: Mutex::new(responses.iter().map(|s| s.to_string()).collect()),
                delay: Duration::ZERO,
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }
    }

    #[async_trait::async_trait]
    impl ModelProvider for ScriptedProvider {
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let mut queue = self.responses.lock().unwrap();
            let response = if queue.len() > 1 {
                queue.pop_front().unwrap()
            } else {
                queue[0].clone()
            };
            Ok(CompletionResponse {
                content: response,
                usage: Default::default(),
            })
        }

        async fn stream(
            &self,
            _req: CompletionRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
            unimplemented!("编排器只使用 complete")
        }

        fn name(&self) -> &str {
            "scripted"
        }

        fn max_context(&self) -> usize {
            8192
        }
    }

    /// 测试事件收集器：事件序列化为 JSON 存入内存，供事件流断言。
    #[derive(Default, Clone)]
    struct CollectSink(Arc<Mutex<Vec<serde_json::Value>>>);

    impl CollectSink {
        fn events(&self) -> Vec<serde_json::Value> {
            self.0.lock().unwrap().clone()
        }

        /// 按顺序取出某状态的步骤名。
        fn step_names(&self, step_status: &str) -> Vec<String> {
            self.events()
                .iter()
                .filter(|v| v["type"] == "step" && v["status"] == step_status)
                .filter_map(|v| v["name"].as_str().map(str::to_string))
                .collect()
        }

        fn has_event(&self, event_type: &str, status: &str) -> bool {
            self.events()
                .iter()
                .any(|v| v["type"] == event_type && (status.is_empty() || v["status"] == status))
        }
    }

    impl EventSink for CollectSink {
        fn emit(&self, event: XaimEvent) -> Result<()> {
            self.0.lock().unwrap().push(serde_json::to_value(&event)?);
            Ok(())
        }
    }

    fn task_status(store: &Store, task_id: i64) -> TaskStatus {
        let conn = store.conn();
        store::task::get_task(&conn, task_id)
            .unwrap()
            .expect("任务应存在")
            .status
    }

    #[tokio::test]
    async fn full_flow_reaches_waiting_confirm() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let (store, project_id) = setup_project(dir.path());
        let sink = CollectSink::default();
        // 调用顺序：意图 x1 + 计划 x1 + 候选 x2（去重后剩 1 个）+ 置信度 x1
        let provider = Arc::new(ScriptedProvider::new(&[
            INTENT_JSON,
            PLAN_JSON,
            FIX_DIFF,
            FIX_DIFF,
            CONFIDENCE_JSON,
        ]));
        let config = OrchestratorConfig {
            candidate_count: 2,
            ..Default::default()
        };
        let orchestrator =
            Orchestrator::with_sink(store.clone(), config, Arc::new(sink.clone()), provider);

        let task_id = orchestrator
            .run_task(project_id, "修复 add 函数的语法错误".to_string())
            .await
            .unwrap();

        // 任务停在等待确认，意图与计划已入库
        let task = {
            let conn = store.conn();
            store::task::get_task(&conn, task_id).unwrap().unwrap()
        };
        assert_eq!(task.status, TaskStatus::WaitingConfirm);
        assert!(task
            .intent
            .as_deref()
            .is_some_and(|s| s.contains("fix_bug")));
        assert!(task.plan.as_deref().is_some_and(|s| s.contains("modify")));

        // 最优补丁已记录且未应用；工作区保持原始状态
        let patches = {
            let conn = store.conn();
            store::patch::list_patches_by_task(&conn, task_id).unwrap()
        };
        assert_eq!(patches.len(), 1, "两个相同候选应去重为一个");
        assert!(!patches[0].applied);
        assert!(patches[0].score.is_some());
        assert_eq!(lib_content(dir.path()), BROKEN_LIB);

        // 事件流：八个步骤依次 running/done，含补丁事件，尚无终态事件
        let expected_steps = [
            "解析意图",
            "检查索引",
            "检索相关文件",
            "生成计划",
            "生成候选",
            "检查补丁",
            "评估候选",
            "等待确认",
        ];
        assert_eq!(sink.step_names("running"), expected_steps);
        assert_eq!(sink.step_names("done"), expected_steps);
        assert!(sink.has_event("patch", ""), "应推送最优补丁事件");
        assert!(!sink.has_event("done", ""), "run_task 不应发终态事件");
        assert!(!sink.has_event("error", ""), "正常流程不应有错误事件");
    }

    #[tokio::test]
    async fn confirm_applies_patch_without_committing_and_finishes() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let (store, project_id) = setup_project(dir.path());
        let sink = CollectSink::default();
        let provider = Arc::new(ScriptedProvider::new(&[
            INTENT_JSON,
            PLAN_JSON,
            FIX_DIFF,
            FIX_DIFF,
            CONFIDENCE_JSON,
        ]));
        let orchestrator = Orchestrator::with_sink(
            store.clone(),
            OrchestratorConfig::default(),
            Arc::new(sink.clone()),
            provider,
        );

        let task_id = orchestrator
            .run_task(project_id, "修复 add 函数的语法错误".to_string())
            .await
            .unwrap();
        orchestrator.confirm_task(task_id).await.unwrap();

        let task = {
            let conn = store.conn();
            store::task::get_task(&conn, task_id).unwrap().unwrap()
        };
        assert_eq!(task.status, TaskStatus::Done);
        assert!(task.finished_at.is_some(), "完成时间应被记录");

        let patches = {
            let conn = store.conn();
            store::patch::list_patches_by_task(&conn, task_id).unwrap()
        };
        assert!(patches[0].applied, "补丁应标记为已应用");
        assert_eq!(lib_content(dir.path()), FIXED_LIB, "补丁应已落盘");

        assert!(
            !dir.path().join(".git").exists(),
            "确认不应自动创建 Git 仓库"
        );
        assert!(sink.has_event("done", "done"), "应推送完成事件");
    }

    #[tokio::test]
    async fn confirm_rejects_task_not_waiting_for_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _project_id) = setup_project(dir.path());
        let task_id = {
            let conn = store.conn();
            store::task::insert_task(&conn, 1, "p").unwrap()
        };
        let orchestrator = Orchestrator::with_sink(
            store,
            OrchestratorConfig::default(),
            Arc::new(CollectSink::default()),
            Arc::new(MockProvider::new("unused")),
        );
        let error = orchestrator.confirm_task(task_id).await.unwrap_err();
        assert!(matches!(error, XaimError::Config(_)), "实际 {error:?}");
    }

    #[tokio::test]
    async fn cancel_stops_task_at_next_step_boundary() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let (store, project_id) = setup_project(dir.path());
        let sink = CollectSink::default();
        // 每次模型调用 300ms：取消发生在第一次调用进行中，下一个边界生效
        let provider = Arc::new(
            ScriptedProvider::new(&[INTENT_JSON, PLAN_JSON, FIX_DIFF, FIX_DIFF, CONFIDENCE_JSON])
                .with_delay(Duration::from_millis(300)),
        );
        let orchestrator = Orchestrator::with_sink(
            store.clone(),
            OrchestratorConfig::default(),
            Arc::new(sink.clone()),
            provider,
        );

        let handle = tokio::spawn({
            let orchestrator = orchestrator.clone();
            async move {
                orchestrator
                    .run_task(project_id, "修复 add".to_string())
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        orchestrator.cancel().await;

        let result = handle.await.unwrap();
        assert!(
            matches!(result, Err(XaimError::Cancelled)),
            "实际 {result:?}"
        );
        assert_eq!(
            task_status(&store, task_id_of(&store)),
            TaskStatus::Cancelled
        );
        assert!(sink.has_event("done", "cancelled"), "应推送取消事件");
        assert!(
            !sink.has_event("error", ""),
            "取消属正常控制流，不应有错误事件"
        );
        // 取消发生在解析意图之后：工作区未被改动
        assert_eq!(lib_content(dir.path()), BROKEN_LIB);
    }

    /// 取最近创建的任务 id（测试辅助：内存库中只有一条任务）。
    fn task_id_of(store: &Store) -> i64 {
        let conn = store.conn();
        store::task::list_tasks(&conn, 1, 10).unwrap()[0].id
    }

    #[tokio::test]
    async fn step_failure_marks_task_failed_without_crash() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let (store, project_id) = setup_project(dir.path());
        let sink = CollectSink::default();
        // 意图解析 3 次都拿到垃圾输出 -> 步骤失败
        let provider = Arc::new(ScriptedProvider::new(&["这不是 JSON"]));
        let orchestrator = Orchestrator::with_sink(
            store.clone(),
            OrchestratorConfig::default(),
            Arc::new(sink.clone()),
            provider,
        );

        let error = orchestrator
            .run_task(project_id, "修复 add".to_string())
            .await
            .unwrap_err();
        assert!(matches!(error, XaimError::Model(_)), "实际 {error:?}");
        assert_eq!(task_status(&store, task_id_of(&store)), TaskStatus::Failed);
        assert!(sink.has_event("error", ""), "应推送错误事件");
        assert!(sink.has_event("done", "failed"), "应推送失败终态事件");
        assert_eq!(sink.step_names("failed"), ["解析意图"]);
        let task_id = task_id_of(&store);
        let conn = store.conn();
        let steps = store::task::list_steps(&conn, task_id).unwrap();
        assert!(steps
            .iter()
            .any(|step| step.step_type == "解析意图" && step.content == "failed"));
        assert_eq!(lib_content(dir.path()), BROKEN_LIB);
    }

    #[tokio::test]
    async fn step_timeout_marks_task_failed() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let (store, project_id) = setup_project(dir.path());
        let sink = CollectSink::default();
        // 每次调用 1.5s，步骤超时 1s：解析意图必然超时
        let provider =
            Arc::new(ScriptedProvider::new(&[INTENT_JSON]).with_delay(Duration::from_millis(1500)));
        let config = OrchestratorConfig {
            step_timeout_sec: 1,
            ..Default::default()
        };
        let orchestrator =
            Orchestrator::with_sink(store.clone(), config, Arc::new(sink.clone()), provider);

        let error = orchestrator
            .run_task(project_id, "修复 add".to_string())
            .await
            .unwrap_err();
        assert!(matches!(error, XaimError::Timeout(_)), "实际 {error:?}");
        assert_eq!(task_status(&store, task_id_of(&store)), TaskStatus::Failed);
        assert!(sink.has_event("done", "failed"));
        assert_eq!(sink.step_names("failed"), ["解析意图"]);
    }

    #[tokio::test]
    async fn empty_project_creates_html_after_correcting_output_and_can_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let (store, project_id) = setup_project(dir.path());
        let files = r#"{"files":[{"path":"web/index.html","content":"<!doctype html>\n<html><svg></svg></html>\n"}]}"#;
        let provider = Arc::new(ScriptedProvider::new(&[
            r#"{"action":"add_feature","target":"web/index.html"}"#,
            r#"{"steps":[{"action":"modify","file":"web/index.html","reason":"新建动画页面"}],"files_to_modify":["web/index.html"],"risks":[]}"#,
            "<html><svg></svg></html>",
            files,
            CONFIDENCE_JSON,
        ]));
        let sink = CollectSink::default();
        let orchestrator = Orchestrator::with_sink(
            store.clone(),
            OrchestratorConfig {
                candidate_count: 1,
                ..Default::default()
            },
            Arc::new(sink.clone()),
            provider,
        );
        let task_id = orchestrator
            .run_task(project_id, "创建一个 HTML 动画页面".into())
            .await
            .unwrap();
        assert_eq!(task_status(&store, task_id), TaskStatus::WaitingConfirm);
        let target = dir.path().join("web/index.html");
        assert!(
            !target.exists(),
            "evaluation must restore absent target before confirmation"
        );
        orchestrator.confirm_task(task_id).await.unwrap();
        assert_eq!(task_status(&store, task_id), TaskStatus::Done);
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "<!doctype html>\n<html><svg></svg></html>\n"
        );
        orchestrator.rollback_task(task_id).await.unwrap();
        assert_eq!(task_status(&store, task_id), TaskStatus::RolledBack);
        assert!(!target.exists());
    }

    #[tokio::test]
    #[ignore = "Explicit live model check; uses the configured API and bills model tokens"]
    async fn live_new_html_workflow() {
        let database = std::env::var("XAIM_LIVE_MODEL_DATABASE").expect("explicit database path");
        let conn = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let config_json: String = conn
            .query_row(
                "SELECT value FROM cache WHERE key='app:config'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let config: crate::types::Config = serde_json::from_str(&config_json).unwrap();
        let prompt: String = conn
            .query_row(
                "SELECT prompt FROM tasks ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(conn);
        let root = tempfile::tempdir().unwrap();
        let log_database = std::env::var("XAIM_LIVE_LOG_DATABASE")
            .expect("explicit persistent log database path, separate from desktop database");
        assert!(
            !Path::new(&log_database).exists(),
            "live log database must be a new file"
        );
        assert_ne!(
            std::path::Path::new(&log_database),
            std::path::Path::new(&std::env::var("XAIM_LIVE_MODEL_DATABASE").unwrap())
        );
        let store = Arc::new(Store::new(Path::new(&log_database)).unwrap());
        let project_id = store::project::insert_project(
            &store.conn(),
            root.path().to_str().unwrap(),
            "live-test",
        )
        .unwrap();
        let runtime = crate::commands::task::orchestrator_config_from(config.clone()).unwrap();
        let provider = crate::model::create_provider(&runtime.model).unwrap();
        let sink = Arc::new(CollectSink::default());
        let orchestrator =
            Orchestrator::with_sink(store.clone(), runtime, sink.clone(), Arc::from(provider));
        let outcome = orchestrator.run_task(project_id, prompt).await;
        // Print protocol metadata only, never source, keys or private reasoning.
        for event in sink.events() {
            if event["type"] == "modelResponse" {
                println!(
                    "Live response: round={}, finish={}, ms={}, tools={}",
                    event["round"], event["finishReason"], event["durationMs"], event["toolCalls"]
                );
            }
        }
        let task_id = outcome.unwrap();
        assert_eq!(task_status(&store, task_id), TaskStatus::WaitingConfirm);
        let patches = {
            let conn = store.conn();
            store::patch::list_patches_by_task(&conn, task_id).unwrap()
        };
        assert_eq!(patches.len(), 1);
        assert!(!root.path().join(&patches[0].file_path).exists());
        orchestrator.confirm_task(task_id).await.unwrap();
        let target = root.path().join(&patches[0].file_path);
        let html = fs::read_to_string(&target).unwrap();
        assert!(html.contains("<html") && html.contains("<svg"));
        if let Ok(output) = std::env::var("XAIM_LIVE_HTML_OUTPUT") {
            fs::write(output, &html).unwrap();
        }
        println!(
            "Live workflow passed: model={}, file={}, bytes={}",
            config.model.name,
            patches[0].file_path,
            html.len()
        );
        orchestrator.rollback_task(task_id).await.unwrap();
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn missing_project_returns_error_without_task() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _project_id) = setup_project(dir.path());
        let orchestrator = Orchestrator::with_sink(
            store.clone(),
            OrchestratorConfig::default(),
            Arc::new(CollectSink::default()),
            Arc::new(MockProvider::new("unused")),
        );
        let error = orchestrator
            .run_task(999, "任意".to_string())
            .await
            .unwrap_err();
        assert!(matches!(error, XaimError::Config(_)), "实际 {error:?}");
        let tasks = {
            let conn = store.conn();
            store::task::list_tasks(&conn, 999, 10).unwrap()
        };
        assert!(tasks.is_empty(), "不存在的项目不应创建任务");
    }

    #[test]
    fn orchestrator_config_defaults_are_sane() {
        let config = OrchestratorConfig::default();
        assert_eq!(config.candidate_count, 1);
        assert_eq!(config.step_timeout_sec, 600);
        assert_eq!(config.context_top_files, 8);
        assert!(!config.require_confirm);
    }

    #[test]
    fn compute_score_follows_formula() {
        let all_pass = [
            verify_result("type_check", true),
            verify_result("lint", true),
            verify_result("test", true),
            verify_result("build", true),
        ];
        // 全部通过 + 最大 diff：0.4 + 0 + 0.2 + 0.1 + 0.1 * 1.0
        let score = compute_score(100, 100, &all_pass, 1.0);
        assert!((score - 0.8).abs() < 1e-9, "实际 {score}");

        // 更小的 diff 得到尺寸分：0.4 + 0.2 * 0.5 + 0.2 + 0.1 + 0.1
        let score = compute_score(50, 100, &all_pass, 1.0);
        assert!((score - 0.9).abs() < 1e-9, "实际 {score}");

        // 4 条验证通过 3 条（测试失败）：0.4 * 0.75 + 0 + 0 + 0.1 + 0.1 * 0.5
        let half = [
            verify_result("type_check", true),
            verify_result("lint", true),
            verify_result("test", false),
            verify_result("build", true),
        ];
        let score = compute_score(100, 100, &half, 0.5);
        assert!((score - 0.45).abs() < 1e-9, "实际 {score}");
    }

    #[test]
    fn compute_score_edge_cases() {
        // 未检测到任何验证命令：验证分与测试/lint 分均为 0
        let score = compute_score(0, 0, &[], 0.5);
        assert!((score - (0.2 + 0.1 * 0.5)).abs() < 1e-9, "实际 {score}");

        // 越界置信度截断到 [0, 1]：仅配置 test 一条且通过
        // 高置信度：0.4 + 0.2(尺寸分) + 0.2(test) + 0.1(conf=1.0) = 0.9
        // 低置信度：0.4 + 0.2 + 0.2 + 0 = 0.8
        let all_pass = [verify_result("test", true)];
        let high = compute_score(0, 10, &all_pass, 2.0);
        let low = compute_score(0, 10, &all_pass, -1.0);
        assert!((high - 0.9).abs() < 1e-9, "实际 {high}");
        assert!((low - 0.8).abs() < 1e-9, "实际 {low}");
    }

    #[test]
    fn parse_confidence_tolerates_number_string_and_garbage() {
        assert_eq!(parse_confidence(r#"{"confidence": 0.9}"#), Some(0.9));
        assert_eq!(parse_confidence(r#"{"confidence": "0.85"}"#), Some(0.85));
        // 围栏与说明文字
        assert_eq!(
            parse_confidence("评审结果：\n```json\n{\"confidence\": 1.5}\n```"),
            Some(1.0)
        );
        assert_eq!(parse_confidence("完全不是 JSON"), None);
        assert_eq!(parse_confidence(r#"{"score": 0.9}"#), None);
    }

    #[test]
    fn backups_restore_workspace_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // 已存在文件：恢复原内容
        write_file(root, "src/lib.rs", "original");
        let backup = backup_file(root, "src/lib.rs").unwrap();
        assert!(matches!(backup, FileBackup::Existing(_)));
        fs::write(root.join("src/lib.rs"), "被补丁覆盖").unwrap();
        restore_file(root, "src/lib.rs", &backup).unwrap();
        assert_eq!(lib_content(root), "original");

        // 原本不存在的文件：回滚即删除
        let backup = backup_file(root, "src/new.rs").unwrap();
        assert!(matches!(backup, FileBackup::Absent));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/new.rs"), "补丁新建").unwrap();
        restore_file(root, "src/new.rs", &backup).unwrap();
        assert!(!root.join("src/new.rs").exists());
    }

    #[test]
    fn backup_rejects_unsafe_paths() {
        let dir = tempfile::tempdir().unwrap();
        let error = backup_file(dir.path(), "../outside.txt").unwrap_err();
        assert!(matches!(error, XaimError::Security(_)), "实际 {error:?}");
    }

    #[tokio::test]
    async fn selects_highest_scoring_candidate_and_restores_workspace() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let ctx = ctx_with(dir.path());
        // 两个候选同一置信度，分数差异完全来自验证结果与 diff 大小
        let provider = MockProvider::new(r#"{"confidence": 0.5}"#);

        let fix = candidate(FIX_DIFF);
        let still_broken = candidate(STILL_BROKEN_DIFF);
        let winner = evaluate_candidates(vec![still_broken.clone(), fix.clone()], &ctx, &provider)
            .await
            .unwrap();

        // 修复候选验证全过：0.4 + 0.2 * 尺寸分 + 0.2 + 0.1 + 0.05
        let max = fix.diff.len().max(still_broken.diff.len()) as f64;
        let expected = 0.4 + 0.2 * (1.0 - fix.diff.len() as f64 / max) + 0.2 + 0.1 + 0.1 * 0.5;
        assert_eq!(winner.diff, FIX_DIFF, "应选中能通过验证的修复候选");
        assert_eq!(
            winner.score.map(|s| (s * 1e9).round() / 1e9),
            Some((expected * 1e9).round() / 1e9)
        );
        // 每个候选评估后都回滚：工作区保持原始坏状态，等待确认阶段
        assert_eq!(lib_content(dir.path()), BROKEN_LIB);
    }

    #[tokio::test]
    async fn apply_failure_is_skipped_and_workspace_untouched() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let ctx = ctx_with(dir.path());
        let provider = MockProvider::new(r#"{"confidence": 0.5}"#);

        let winner = evaluate_candidates(
            vec![candidate(CONFLICT_DIFF), candidate(FIX_DIFF)],
            &ctx,
            &provider,
        )
        .await
        .unwrap();

        assert_eq!(winner.diff, FIX_DIFF, "应用失败的候选应被跳过");
        assert_eq!(lib_content(dir.path()), BROKEN_LIB);
    }

    #[tokio::test]
    async fn confidence_review_failure_falls_back_to_neutral() {
        let dir = tempfile::tempdir().unwrap();
        cargo_project(dir.path(), BROKEN_LIB);
        let ctx = ctx_with(dir.path());
        // 置信度评审不可用：评估仍应完成，只是该项取中性值 0.5
        let provider = MockProvider::new("unused").with_error("模型不可用");

        let winner = evaluate_candidates(vec![candidate(FIX_DIFF)], &ctx, &provider)
            .await
            .unwrap();

        // 单候选：max_diff_size 即自身，尺寸分为 0
        // 0.4(验证全过) + 0 + 0.2(test) + 0.1(lint) + 0.1 * 0.5(中性置信度)
        let expected = 0.4 + 0.2 + 0.1 + 0.1 * NEUTRAL_CONFIDENCE;
        assert!(
            (winner.score.unwrap() - expected).abs() < 1e-9,
            "实际 {}",
            winner.score.unwrap()
        );
        assert_eq!(lib_content(dir.path()), BROKEN_LIB);
    }

    #[tokio::test]
    async fn empty_candidates_return_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with(dir.path());
        let provider = MockProvider::new("unused");
        let error = evaluate_candidates(Vec::new(), &ctx, &provider)
            .await
            .unwrap_err();
        assert!(matches!(error, XaimError::Verify(_)), "实际 {error:?}");
    }

    #[test]
    fn status_labels_cover_terminal_states() {
        assert_eq!(status_label(&TaskStatus::Done), "done");
        assert_eq!(status_label(&TaskStatus::Failed), "failed");
        assert_eq!(status_label(&TaskStatus::Cancelled), "cancelled");
        assert_eq!(status_label(&TaskStatus::RolledBack), "rolledBack");
    }

    #[test]
    fn context_defaults_are_stable() {
        let ctx = ctx_with(Path::new("."));
        assert_eq!(ctx.project_root.to_str(), Some("."));
        assert!(!ctx.require_confirm);
    }
}
