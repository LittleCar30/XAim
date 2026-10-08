//! 任务规划：意图解析、执行计划生成与候选 Patch 生成。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

use crate::config::model::AppConfig;
use crate::errors::{Result, XaimError};
use crate::model::provider::{CompletionRequest, Message, ModelProvider};
use crate::model::router::{pick_model, TaskKind};
use crate::prompts;
use crate::search::context::Context;
use crate::tools::patch::{generate_diff, parse_diff, resolve_in_root, PatchCandidate};

/// 意图解析允许的动作枚举，与 [`prompts::intent::SYSTEM`] 的输出格式一致。
pub const ACTIONS: [&str; 8] = [
    "fix_bug",
    "add_feature",
    "add_test",
    "refactor",
    "review",
    "explain",
    "upgrade",
    "cross_file",
];

/// 计划步骤允许的动作枚举，与 [`prompts::plan::SYSTEM`] 的输出格式一致。
pub const STEP_ACTIONS: [&str; 3] = ["read", "modify", "test"];

/// 从用户任务解析出的结构化意图，对应模型输出 JSON 的五个字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Intent {
    /// 动作类型，取值见 [`ACTIONS`]。
    pub action: String,
    /// 目标文件或模块名；模型不确定时可能为空串。
    pub target: String,
    /// 一句话描述。
    pub description: String,
    /// 用户或推导出的约束。
    pub constraints: Vec<String>,
    /// 建议的验证手段（type / lint / test）。
    pub verify: Vec<String>,
}

/// 执行计划中的一个步骤。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    /// 步骤动作，取值见 [`STEP_ACTIONS`]；test 步骤的 `file` 存验证命令。
    pub action: String,
    /// 目标文件（相对项目根）或验证命令。
    pub file: String,
    /// 这一步要做什么、为什么，保证计划可读。
    pub reason: String,
}

/// 依据任务与代码上下文生成的执行计划。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// 按执行顺序排列的步骤。
    pub steps: Vec<PlanStep>,
    /// 所有将被修改的文件（相对项目根）。
    pub files_to_modify: Vec<String>,
    /// 风险标注。
    pub risks: Vec<String>,
}

/// 解析用户任务的意图：构建提示词 -> 调用模型 -> 解析并校验 JSON。
///
/// `lang` 为项目主语言（如 `"rust"`，无法判定时传 `"未知"`），由
/// [`crate::index::lang::detect_project_language`] 检测后经编排器传入。
/// 模型按路由默认走 Fast 档（[`TaskKind::Intent`]）；需要指定模型时用
/// [`parse_intent_with_model`]。模型输出不是合法意图 JSON 时最多重试 2 次
/// （共 3 次调用），仍失败则返回 [`XaimError::Model`]。
pub async fn parse_intent(task: &str, lang: &str, provider: &dyn ModelProvider) -> Result<Intent> {
    let choice = pick_model(TaskKind::Intent, &AppConfig::default());
    parse_intent_with_model(task, lang, choice.name(), provider).await
}

/// [`parse_intent`] 的显式模型版本，模型名填入 [`CompletionRequest::model`]。
pub async fn parse_intent_with_model(
    task: &str,
    lang: &str,
    model: &str,
    provider: &dyn ModelProvider,
) -> Result<Intent> {
    let messages = vec![
        Message::system(prompts::intent::SYSTEM),
        Message::user(prompts::intent::render(task, lang)),
    ];
    complete_json_with_retries(provider, model, messages, parse_intent_json, "意图解析失败").await
}

/// 依据任务与代码上下文生成执行计划：构建提示词 -> 调用模型 -> 解析并校验 JSON。
///
/// 模型按路由默认走 Strong 档（[`TaskKind::Generate`]，规划属生成类任务）；
/// 需要指定模型时用 [`generate_plan_with_model`]。模型输出不是合法计划 JSON 时
/// 最多重试 2 次。校验：步骤非空且 `action` 属于 [`STEP_ACTIONS`]、每个步骤的
/// action/file 非空、包含 modify 步骤时 `files_to_modify` 不得为空。
pub async fn generate_plan(
    task: &str,
    context: &Context,
    provider: &dyn ModelProvider,
) -> Result<Plan> {
    let choice = pick_model(TaskKind::Generate, &AppConfig::default());
    generate_plan_with_model(task, context, choice.name(), provider).await
}

/// [`generate_plan`] 的显式模型版本，模型名填入 [`CompletionRequest::model`]。
pub async fn generate_plan_with_model(
    task: &str,
    context: &Context,
    model: &str,
    provider: &dyn ModelProvider,
) -> Result<Plan> {
    let messages = vec![
        Message::system(prompts::plan::SYSTEM),
        Message::user(prompts::plan::build(task, context)),
    ];
    complete_json_with_retries(provider, model, messages, parse_plan_json, "计划生成失败").await
}

/// 重试时附加的纠正提示。
const RETRY_HINT: &str =
    "上一次输出无法解析为合法的 JSON。请严格只输出一个 JSON 对象，不要包含解释、markdown 代码块标记或其他任何内容。";

/// 并行生成 `n` 个候选 Patch：按 [`prompts::modify`] 的提示词并行调用 `n` 次
/// 模型（温度 0.1/0.2/0.3 递增，见 [`candidate_temperature`]），用
/// [`parse_diff`] 解析各次输出并按（文件路径, diff 文本）去重后返回。
///
/// 隔离规则：每个候选独立调用、独立解析，单个候选的传输错误或无效输出
/// 只淘汰该候选，不影响其余；`n` 为 0 时直接返回空向量；全部候选失败时
/// 返回 [`XaimError::Model`]；全部输出格式无效时额外进行一次格式纠正。
pub async fn generate_candidates(
    task: &str,
    plan: &Plan,
    context: &Context,
    provider: &dyn ModelProvider,
    n: usize,
) -> Result<Vec<PatchCandidate>> {
    let choice = pick_model(TaskKind::Generate, &AppConfig::default());
    generate_candidates_with_model(task, plan, context, choice.name(), provider, n).await
}

/// [`generate_candidates`] 的显式模型版本，模型名填入 [`CompletionRequest::model`]。
pub async fn generate_candidates_with_model(
    task: &str,
    plan: &Plan,
    context: &Context,
    model: &str,
    provider: &dyn ModelProvider,
    n: usize,
) -> Result<Vec<PatchCandidate>> {
    generate_candidates_inner(task, plan, context, model, provider, n, None).await
}

pub async fn generate_candidates_in_workspace(
    task: &str,
    plan: &Plan,
    context: &Context,
    model: &str,
    provider: &dyn ModelProvider,
    n: usize,
    root: &Path,
) -> Result<Vec<PatchCandidate>> {
    generate_candidates_inner(task, plan, context, model, provider, n, Some(root)).await
}

#[allow(clippy::too_many_arguments)]
async fn generate_candidates_inner(
    task: &str,
    plan: &Plan,
    context: &Context,
    model: &str,
    provider: &dyn ModelProvider,
    n: usize,
    root: Option<&Path>,
) -> Result<Vec<PatchCandidate>> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let plan_text = format_plan(plan);
    let new_paths = match root {
        Some(root) => planned_new_paths(plan, root)?,
        None => Vec::new(),
    };
    let new_files_only = !new_paths.is_empty() && new_paths.len() == plan.files_to_modify.len();
    let system = if new_files_only {
        prompts::modify::NEW_FILES_SYSTEM
    } else {
        prompts::modify::SYSTEM
    };
    let user_message = if new_files_only {
        format!(
            "任务：{task}\n计划：\n{plan_text}\n上下文：{}\n这些路径已确认不存在：{}\n只输出包含完整文件内容的 JSON。",
            prompts::plan::format_context(context),
            new_paths.join(", ")
        )
    } else {
        prompts::modify::build(task, &plan_text, context)
    };

    let attempts = (0..n).map(|index| {
        let user_message = user_message.clone();
        let new_paths = &new_paths;
        async move {
            let mut request = CompletionRequest::new(
                model,
                vec![Message::system(system), Message::user(user_message)],
            );
            request.temperature = candidate_temperature(index);
            let response = provider.complete(request).await?;
            Ok::<_, XaimError>((
                response.content.clone(),
                parse_candidate_response(&response.content, new_paths),
            ))
        }
    });
    let results = futures::future::join_all(attempts).await;

    let mut candidates: Vec<PatchCandidate> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut invalid_output = None;
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for result in results {
        match result {
            Ok((_, Ok(parsed))) => {
                for candidate in parsed {
                    if seen.insert((candidate.file_path.clone(), candidate.diff.clone())) {
                        candidates.push(candidate);
                    }
                }
            }
            Ok((output, Err(error))) => {
                failures.push(error.to_string());
                invalid_output = Some(output);
            }
            Err(error) => failures.push(error.to_string()),
        }
    }

    // One correction call only when the initial batch yielded no usable output.
    let mut corrected = false;
    if candidates.is_empty() {
        if let Some(output) = invalid_output {
            corrected = true;
            let reason = failures.last().map(String::as_str).unwrap_or("格式无效");
            let request = CompletionRequest::new(
                model,
                vec![
                    Message::system(system),
                    Message::user(user_message),
                    Message::assistant(output),
                    Message::user(if new_files_only {
                        format!(
                        "上次输出无法解析：{reason}。请只输出 {{\"files\":[{{\"path\":\"计划路径\",\"content\":\"完整源码\"}}]}}。不要输出 diff、解释或占位符。"
                    )
                    } else {
                        format!(
                        "上次输出无法作为补丁解析：{reason}。请重新输出完整、合法的 unified diff。\
                     新文件必须使用 --- /dev/null、+++ b/项目相对路径、@@ -0,0 +1,N @@，\
                     所有正文行前加 + 并准确计算 N。不要直接输出 HTML 或代码块。"
                    )
                    }),
                ],
            );
            match provider.complete(request).await {
                Ok(response) => match parse_candidate_response(&response.content, &new_paths) {
                    Ok(parsed) => candidates.extend(parsed),
                    Err(error) => failures.push(error.to_string()),
                },
                Err(error) => failures.push(error.to_string()),
            }
        }
    }
    if candidates.is_empty() {
        let reason = failures.last().map(String::as_str).unwrap_or("未知原因");
        let correction = if corrected {
            "，格式纠正后仍无有效补丁"
        } else {
            ""
        };
        return Err(XaimError::Model(format!(
            "候选生成失败：{n} 个候选均未产出有效补丁{correction}。最后一次原因：{reason}"
        )));
    }
    Ok(candidates)
}

fn planned_new_paths(plan: &Plan, root: &Path) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for path in &plan.files_to_modify {
        let target = resolve_in_root(root, path)?;
        match std::fs::symlink_metadata(target) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => paths.push(path.clone()),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(paths)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedFiles {
    files: Vec<GeneratedFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedFile {
    path: String,
    content: String,
}

fn parse_candidate_response(output: &str, new_paths: &[String]) -> Result<Vec<PatchCandidate>> {
    if output.trim().is_empty() {
        return Err(XaimError::Model("模型返回空正文，未生成代码".into()));
    }
    let trimmed = output.trim();
    if new_paths.is_empty()
        || !(trimmed.starts_with('{')
            || trimmed.starts_with("```json")
            || trimmed.starts_with("```\n{"))
    {
        return parse_candidate_output(output);
    }
    let json = trimmed
        .strip_prefix("```json\n")
        .or_else(|| trimmed.strip_prefix("```\n"))
        .and_then(|text| text.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    let files: GeneratedFiles = serde_json::from_str(json)
        .map_err(|error| XaimError::Model(format!("新文件 JSON 格式无效: {error}")))?;
    if files.files.is_empty() {
        return Err(XaimError::Model("新文件输出 files 为空".into()));
    }
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for file in files.files {
        if !new_paths.contains(&file.path) || !seen.insert(file.path.clone()) {
            return Err(XaimError::Security(format!(
                "模型输出路径未获准新建或重复: {}",
                file.path
            )));
        }
        if file.content.trim().is_empty() {
            return Err(XaimError::Model(format!("{}: 新文件内容为空", file.path)));
        }
        let diff = generate_diff("", &file.content, &file.path);
        let (_, body) = diff
            .split_once('\n')
            .ok_or_else(|| XaimError::Model("无法生成新文件补丁".into()))?;
        candidates.extend(parse_candidate_output(&format!("--- /dev/null\n{body}"))?);
    }
    Ok(candidates)
}

fn parse_candidate_output(output: &str) -> Result<Vec<PatchCandidate>> {
    let candidates = parse_diff(output)?;
    if candidates.is_empty() {
        return Err(XaimError::PatchConflict("输出中未找到 diff 块".into()));
    }
    for candidate in &candidates {
        if !candidate.diff.lines().any(|line| line.starts_with("@@ ")) {
            return Err(XaimError::PatchConflict(format!(
                "{}: diff 缺少 hunk",
                candidate.file_path
            )));
        }
        diffy::Patch::from_str(&candidate.diff).map_err(|error| {
            XaimError::PatchConflict(format!("{}: diff 格式无效: {error}", candidate.file_path))
        })?;
    }
    Ok(candidates)
}

/// 第 `index` 个候选（从 0 起）的采样温度：0.1 起步按 0.1 递增，
/// 3 个以上时封顶 0.3，兼顾候选间差异与输出稳定性。
fn candidate_temperature(index: usize) -> f32 {
    (0.1 * (index as f32 + 1.0)).min(0.3)
}

/// 把计划渲染成人可读的提示词文本：编号步骤 + 待修改文件 + 风险。
fn format_plan(plan: &Plan) -> String {
    let mut text = String::new();
    for (index, step) in plan.steps.iter().enumerate() {
        text.push_str(&format!(
            "{}. [{}] {} — {}\n",
            index + 1,
            step.action,
            step.file,
            step.reason
        ));
    }
    if !plan.files_to_modify.is_empty() {
        text.push_str(&format!(
            "待修改文件：{}\n",
            plan.files_to_modify.join(", ")
        ));
    }
    if !plan.risks.is_empty() {
        text.push_str(&format!("风险：{}\n", plan.risks.join("；")));
    }
    text
}

/// 调用模型并解析 JSON 输出的公共骨架：构建请求 -> 调用 -> 解析，
/// 解析或校验失败时附加 [`RETRY_HINT`] 重试，最多重试 2 次（共 3 次调用）；
/// 传输层错误（网络、鉴权、限流）不属解析失败，直接上抛。
/// `fail_prefix` 用于组装最终错误信息（如 "意图解析失败"）。
async fn complete_json_with_retries<T, F>(
    provider: &dyn ModelProvider,
    model: &str,
    base_messages: Vec<Message>,
    parse: F,
    fail_prefix: &str,
) -> Result<T>
where
    T: Send,
    F: Fn(&str) -> std::result::Result<T, String> + Send,
{
    const MAX_ATTEMPTS: usize = 3;

    let mut last_error = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        let mut messages = base_messages.clone();
        if attempt > 0 {
            messages.push(Message::user(RETRY_HINT));
        }
        let response = provider
            .complete(CompletionRequest::new(model, messages))
            .await?;
        match parse(&response.content) {
            Ok(value) => return Ok(value),
            Err(error) => last_error = error,
        }
    }
    Err(XaimError::Model(format!(
        "{fail_prefix}：重试 2 次后模型仍未输出合法的 JSON。最后一次原因：{last_error}"
    )))
}

/// 模型输出意图 JSON 的宽松形态：除 action 必填外，其余字段缺失或为 null 均落为空值。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawIntent {
    action: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    constraints: Option<Vec<String>>,
    #[serde(default)]
    verify: Option<Vec<String>>,
}

/// 把模型输出文本解析成 [`Intent`]，失败时返回人可读的原因。
///
/// 兼容 markdown 代码块与前后缀说明文字：截取首个 `{` 到最后一个 `}`
/// 之间的内容再解析。校验规则：`action` 必须属于 [`ACTIONS`]。
fn parse_intent_json(text: &str) -> std::result::Result<Intent, String> {
    let json_text = extract_json_object(text)
        .ok_or_else(|| format!("输出中不包含 JSON 对象。原文：{}", snippet(text)))?;
    let raw: RawIntent = serde_json::from_str(json_text)
        .map_err(|error| format!("JSON 解析失败：{error}。原文：{}", snippet(json_text)))?;

    let action = raw.action.trim();
    if !ACTIONS.contains(&action) {
        return Err(format!(
            "action \"{action}\" 不在允许的枚举中，应为 {} 之一",
            ACTIONS.join(" / ")
        ));
    }
    Ok(Intent {
        action: action.to_string(),
        target: raw.target.unwrap_or_default(),
        description: raw.description.unwrap_or_default(),
        constraints: raw.constraints.unwrap_or_default(),
        verify: raw.verify.unwrap_or_default(),
    })
}

/// 模型输出计划 JSON 的宽松形态。模型按提示词以 snake_case 输出
/// `files_to_modify`，经 `alias` 兼容；缺失字段落为空值。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPlan {
    #[serde(default)]
    steps: Option<Vec<RawPlanStep>>,
    #[serde(default, alias = "files_to_modify")]
    files_to_modify: Option<Vec<String>>,
    #[serde(default)]
    risks: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPlanStep {
    action: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// 把模型输出文本解析成 [`Plan`]，失败时返回人可读的原因。
///
/// 兼容 markdown 代码块与前后缀说明文字。校验规则见 [`generate_plan`]。
fn parse_plan_json(text: &str) -> std::result::Result<Plan, String> {
    let json_text = extract_json_object(text)
        .ok_or_else(|| format!("输出中不包含 JSON 对象。原文：{}", snippet(text)))?;
    let raw: RawPlan = serde_json::from_str(json_text)
        .map_err(|error| format!("JSON 解析失败：{error}。原文：{}", snippet(json_text)))?;

    let raw_steps = raw.steps.ok_or_else(|| "计划缺少 steps 字段".to_string())?;
    if raw_steps.is_empty() {
        return Err("计划没有任何步骤".to_string());
    }
    let mut steps = Vec::with_capacity(raw_steps.len());
    for (index, raw_step) in raw_steps.into_iter().enumerate() {
        let action = raw_step.action.trim().to_string();
        if action.is_empty() {
            return Err(format!("第 {} 个步骤缺少 action", index + 1));
        }
        if !STEP_ACTIONS.contains(&action.as_str()) {
            return Err(format!(
                "第 {} 个步骤的 action \"{action}\" 不在允许的枚举中，应为 {} 之一",
                index + 1,
                STEP_ACTIONS.join(" / ")
            ));
        }
        let file = raw_step.file.unwrap_or_default().trim().to_string();
        if file.is_empty() {
            return Err(format!(
                "第 {} 个步骤缺少 file（test 步骤填验证命令）",
                index + 1
            ));
        }
        steps.push(PlanStep {
            action,
            file,
            reason: raw_step.reason.unwrap_or_default(),
        });
    }

    let files_to_modify = raw.files_to_modify.unwrap_or_default();
    if steps.iter().any(|step| step.action == "modify") && files_to_modify.is_empty() {
        return Err(
            "计划包含 modify 步骤但 files_to_modify 为空，必须列出所有要修改的文件".to_string(),
        );
    }

    Ok(Plan {
        steps,
        files_to_modify,
        risks: raw.risks.unwrap_or_default(),
    })
}

/// 从模型输出中截取 JSON 对象文本：首个 `{` 到最后一个 `}` 之间。
pub(crate) fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (start <= end).then_some(&text[start..=end])
}

/// 错误信息里携带的原文片段，超长截断，避免错误刷屏。
fn snippet(text: &str) -> String {
    const MAX_CHARS: usize = 200;
    let mut result: String = text.chars().take(MAX_CHARS).collect();
    if text.chars().count() > MAX_CHARS {
        result.push('…');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::provider::{CompletionResponse, MockProvider, Usage};
    use futures::Stream;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// 依次返回预设响应的测试 provider：响应耗尽后重复最后一条；
    /// 配置 `error` 时所有调用都报错，配置 `fail_first` 时前 N 次调用报错。
    /// 记录每次请求与调用次数，用于重试与并行断言。
    struct SeqProvider {
        responses: Mutex<Vec<String>>,
        requests: Mutex<Vec<RecordedRequest>>,
        error: Option<String>,
        fail_first: usize,
        calls: AtomicUsize,
    }

    struct RecordedRequest {
        model: String,
        messages: Vec<Message>,
        temperature: f32,
    }

    impl SeqProvider {
        fn new(responses: &[&str]) -> Self {
            Self::of(responses.iter().map(|s| s.to_string()).collect())
        }

        fn of(responses: Vec<String>) -> Self {
            Self {
                responses: Mutex::new(responses),
                requests: Mutex::new(Vec::new()),
                error: None,
                fail_first: 0,
                calls: AtomicUsize::new(0),
            }
        }

        fn with_error(mut self, message: impl Into<String>) -> Self {
            self.error = Some(message.into());
            self
        }

        fn with_fail_first(mut self, count: usize) -> Self {
            self.fail_first = count;
            self
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn requested_messages(&self, index: usize) -> Vec<Message> {
            self.requests.lock().unwrap()[index].messages.clone()
        }

        fn requested_model(&self, index: usize) -> String {
            self.requests.lock().unwrap()[index].model.clone()
        }

        fn requested_temperature(&self, index: usize) -> f32 {
            self.requests.lock().unwrap()[index].temperature
        }
    }

    #[async_trait::async_trait]
    impl ModelProvider for SeqProvider {
        async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().unwrap().push(RecordedRequest {
                model: req.model.clone(),
                messages: req.messages.clone(),
                temperature: req.temperature,
            });
            if call_index < self.fail_first {
                return Err(XaimError::Model(format!(
                    "第 {} 次调用失败",
                    call_index + 1
                )));
            }
            if let Some(message) = &self.error {
                return Err(XaimError::Model(message.clone()));
            }
            let mut queue = self.responses.lock().unwrap();
            let response = if queue.len() > 1 {
                queue.remove(0)
            } else {
                queue[0].clone()
            };
            Ok(CompletionResponse {
                content: response,
                usage: Usage::default(),
            })
        }

        async fn stream(
            &self,
            _req: CompletionRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
            unimplemented!("planner 只使用 complete")
        }

        fn name(&self) -> &str {
            "seq"
        }

        fn max_context(&self) -> usize {
            8192
        }
    }

    fn make_intent(
        action: &str,
        target: &str,
        description: &str,
        constraints: &[&str],
        verify: &[&str],
    ) -> Intent {
        Intent {
            action: action.to_string(),
            target: target.to_string(),
            description: description.to_string(),
            constraints: constraints.iter().map(|s| s.to_string()).collect(),
            verify: verify.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// 20 个意图端到端样例：任务描述 -> 模拟模型输出 -> 期望的完整意图。
    /// 覆盖全部 7 种 action，JSON 形态兼顾紧凑与带空格排版。
    fn intent_samples() -> [(&'static str, &'static str, Intent); 20] {
        [
            (
                "修复登录空指针",
                r#"{"action":"fix_bug","target":"login","description":"修复登录接口的空指针异常","constraints":[],"verify":["type","test"]}"#,
                make_intent(
                    "fix_bug",
                    "login",
                    "修复登录接口的空指针异常",
                    &[],
                    &["type", "test"],
                ),
            ),
            (
                "给 auth 模块补单元测试",
                r#"{"action":"add_test","target":"auth","description":"为 auth 模块补充单元测试","constraints":["覆盖主要分支"],"verify":["test"]}"#,
                make_intent(
                    "add_test",
                    "auth",
                    "为 auth 模块补充单元测试",
                    &["覆盖主要分支"],
                    &["test"],
                ),
            ),
            (
                "把所有 fetch 改成 axios",
                r#"{"action":"cross_file","target":"*","description":"全局替换 fetch 为 axios","constraints":["保持 API 兼容"],"verify":["type","test"]}"#,
                make_intent(
                    "cross_file",
                    "*",
                    "全局替换 fetch 为 axios",
                    &["保持 API 兼容"],
                    &["type", "test"],
                ),
            ),
            (
                "修复 src/utils/date.ts 中的时区换算错误",
                r#"{"action": "fix_bug", "target": "src/utils/date.ts", "description": "修复时区换算错误", "constraints": [], "verify": ["type", "test"]}"#,
                make_intent(
                    "fix_bug",
                    "src/utils/date.ts",
                    "修复时区换算错误",
                    &[],
                    &["type", "test"],
                ),
            ),
            (
                "review 一下 payment 模块的错误处理",
                r#"{"action":"review","target":"payment","description":"审查支付模块的错误处理","constraints":[],"verify":["lint"]}"#,
                make_intent(
                    "review",
                    "payment",
                    "审查支付模块的错误处理",
                    &[],
                    &["lint"],
                ),
            ),
            (
                "解释一下项目的启动流程",
                r#"{"action":"explain","target":"","description":"解释项目启动流程","constraints":[],"verify":[]}"#,
                make_intent("explain", "", "解释项目启动流程", &[], &[]),
            ),
            (
                "把 moment.js 升级到最新版本",
                r#"{"action":"upgrade","target":"package.json","description":"升级 moment.js 到最新版","constraints":[],"verify":["type","test"]}"#,
                make_intent(
                    "upgrade",
                    "package.json",
                    "升级 moment.js 到最新版",
                    &[],
                    &["type", "test"],
                ),
            ),
            (
                "重构 user_service 中过长的 processOrder 函数",
                r#"{"action":"refactor","target":"user_service","description":"拆分 processOrder 函数","constraints":["保持行为不变"],"verify":["type","test"]}"#,
                make_intent(
                    "refactor",
                    "user_service",
                    "拆分 processOrder 函数",
                    &["保持行为不变"],
                    &["type", "test"],
                ),
            ),
            (
                "修复订单金额计算精度丢失的问题",
                r#"{"action":"fix_bug","target":"order","description":"修复金额计算精度丢失","constraints":["不改变数据库表结构"],"verify":["type","test"]}"#,
                make_intent(
                    "fix_bug",
                    "order",
                    "修复金额计算精度丢失",
                    &["不改变数据库表结构"],
                    &["type", "test"],
                ),
            ),
            (
                "给搜索接口补参数校验的单元测试",
                r#"{"action":"add_test","target":"search","description":"为搜索接口补充参数校验测试","constraints":["覆盖边界条件"],"verify":["test"]}"#,
                make_intent(
                    "add_test",
                    "search",
                    "为搜索接口补充参数校验测试",
                    &["覆盖边界条件"],
                    &["test"],
                ),
            ),
            (
                "解释这段 borrow checker 报错的含义",
                r#"{"action":"explain","target":"","description":"解释 borrow checker 报错","constraints":[],"verify":[]}"#,
                make_intent("explain", "", "解释 borrow checker 报错", &[], &[]),
            ),
            (
                "把所有 console.log 替换成 logger.debug",
                r#"{"action":"cross_file","target":"*","description":"全局替换 console.log 为 logger.debug","constraints":["保留原有日志级别语义"],"verify":["lint","test"]}"#,
                make_intent(
                    "cross_file",
                    "*",
                    "全局替换 console.log 为 logger.debug",
                    &["保留原有日志级别语义"],
                    &["lint", "test"],
                ),
            ),
            (
                "review 数据库模块的 SQL 注入风险",
                r#"{"action":"review","target":"database","description":"审查 SQL 注入风险","constraints":[],"verify":[]}"#,
                make_intent("review", "database", "审查 SQL 注入风险", &[], &[]),
            ),
            (
                "把 tauri 从 1.x 升级到 2.x",
                r#"{"action":"upgrade","target":"src-tauri","description":"升级 tauri 到 2.x","constraints":["保持前端 API 兼容"],"verify":["type","test"]}"#,
                make_intent(
                    "upgrade",
                    "src-tauri",
                    "升级 tauri 到 2.x",
                    &["保持前端 API 兼容"],
                    &["type", "test"],
                ),
            ),
            (
                "把回调风格的代码改成 async/await",
                r#"{"action":"refactor","target":"src","description":"回调改 async/await","constraints":[],"verify":["type"]}"#,
                make_intent("refactor", "src", "回调改 async/await", &[], &["type"]),
            ),
            (
                "修复内存泄漏，注意不能改变公开 API",
                r#"{"action":"fix_bug","target":"","description":"修复内存泄漏","constraints":["不改变公开 API"],"verify":["test"]}"#,
                make_intent(
                    "fix_bug",
                    "",
                    "修复内存泄漏",
                    &["不改变公开 API"],
                    &["test"],
                ),
            ),
            (
                "给 auth 模块的登录函数补集成测试",
                r#"{"action":"add_test","target":"auth","description":"补充登录集成测试","constraints":["覆盖登录成功与失败分支"],"verify":["test"]}"#,
                make_intent(
                    "add_test",
                    "auth",
                    "补充登录集成测试",
                    &["覆盖登录成功与失败分支"],
                    &["test"],
                ),
            ),
            (
                "解释 config 模块加载配置的顺序",
                r#"{"action":"explain","target":"config","description":"解释配置加载顺序","constraints":[],"verify":[]}"#,
                make_intent("explain", "config", "解释配置加载顺序", &[], &[]),
            ),
            (
                "升级所有依赖到最新补丁版本",
                r#"{"action":"upgrade","target":"package.json","description":"升级依赖补丁版本","constraints":["只升补丁版本，不引入破坏性变更"],"verify":["type","test","lint"]}"#,
                make_intent(
                    "upgrade",
                    "package.json",
                    "升级依赖补丁版本",
                    &["只升补丁版本，不引入破坏性变更"],
                    &["type", "test", "lint"],
                ),
            ),
            (
                "把 REST 调用统一迁移到新的 api client，保持所有现有接口行为不变",
                r#"{"action":"cross_file","target":"src/api","description":"REST 调用迁移到统一 api client","constraints":["保持所有现有接口行为不变","分批小步提交"],"verify":["type","test"]}"#,
                make_intent(
                    "cross_file",
                    "src/api",
                    "REST 调用迁移到统一 api client",
                    &["保持所有现有接口行为不变", "分批小步提交"],
                    &["type", "test"],
                ),
            ),
        ]
    }

    #[tokio::test]
    async fn parses_20_samples_end_to_end() {
        let samples = intent_samples();
        assert_eq!(samples.len(), 20);
        for (index, (task, response, expected)) in samples.iter().enumerate() {
            let provider = MockProvider::new(*response);
            let parsed = parse_intent(task, "rust", &provider)
                .await
                .unwrap_or_else(|error| panic!("样例 {}（{task}）解析失败: {error}", index + 1));
            assert_eq!(&parsed, expected, "样例 {}（{task}）意图不符", index + 1);
        }
    }

    #[tokio::test]
    async fn sample_outputs_are_directly_parseable_json() {
        for (_, response, _) in intent_samples() {
            let value: serde_json::Value =
                serde_json::from_str(response).expect("样例输出必须是可解析 JSON");
            let object = value.as_object().expect("样例输出必须是 JSON 对象");
            for key in ["action", "target", "description", "constraints", "verify"] {
                assert!(
                    object.contains_key(key),
                    "样例输出缺少字段 {key}: {response}"
                );
            }
        }
    }

    #[tokio::test]
    async fn markdown_fenced_json_is_parsed() {
        let response = "```json\n{\"action\":\"review\",\"target\":\"core\",\"description\":\"代码审查\",\"constraints\":[],\"verify\":[]}\n```";
        let provider = MockProvider::new(response);
        let intent = parse_intent("review core 模块", "go", &provider)
            .await
            .unwrap();
        assert_eq!(intent.action, "review");
        assert_eq!(intent.target, "core");
    }

    #[tokio::test]
    async fn prose_around_json_is_tolerated() {
        let response = "好的，解析结果如下：\n{\"action\":\"explain\",\"target\":\"\",\"description\":\"解释报错\",\"constraints\":[],\"verify\":[]}\n以上就是解析结果。";
        let provider = MockProvider::new(response);
        let intent = parse_intent("解释这个报错", "python", &provider)
            .await
            .unwrap();
        assert_eq!(intent.action, "explain");
        assert_eq!(intent.description, "解释报错");
    }

    #[tokio::test]
    async fn null_fields_default_to_empty() {
        let provider = MockProvider::new(
            r#"{"action": "refactor", "target": null, "description": null, "constraints": null, "verify": null}"#,
        );
        let intent = parse_intent("重构代码", "typescript", &provider)
            .await
            .unwrap();
        assert_eq!(
            intent,
            make_intent("refactor", "", "", &[], &[]),
            "null 字段应落为空值"
        );
    }

    #[tokio::test]
    async fn missing_optional_fields_fall_back_to_defaults() {
        let provider = MockProvider::new(r#"{"action":"fix_bug"}"#);
        let intent = parse_intent("修复崩溃", "rust", &provider).await.unwrap();
        assert_eq!(intent, make_intent("fix_bug", "", "", &[], &[]));
    }

    #[tokio::test]
    async fn action_is_trimmed_before_validation() {
        let provider = MockProvider::new(
            r#"{"action":"  fix_bug  ","target":"a","description":"d","constraints":[],"verify":[]}"#,
        );
        let intent = parse_intent("修复 a", "rust", &provider).await.unwrap();
        assert_eq!(intent.action, "fix_bug");
    }

    #[tokio::test]
    async fn unknown_action_is_rejected_then_retry_succeeds() {
        let provider = SeqProvider::new(&[
            r#"{"action":"bug_fix","target":"a","description":"d","constraints":[],"verify":[]}"#,
            r#"{"action":"fix_bug","target":"a","description":"d","constraints":[],"verify":[]}"#,
        ]);
        let intent = parse_intent("修复 a", "rust", &provider).await.unwrap();
        assert_eq!(intent.action, "fix_bug");
        assert_eq!(provider.call_count(), 2, "非法 action 应触发一次重试");
    }

    #[tokio::test]
    async fn malformed_json_exhausts_two_retries_then_fails() {
        let provider = SeqProvider::new(&["这不是 JSON"]);
        let error = parse_intent("修复 a", "rust", &provider).await.unwrap_err();
        assert!(matches!(error, XaimError::Model(_)), "实际错误: {error:?}");
        assert_eq!(provider.call_count(), 3, "首次 + 重试 2 次，共 3 次调用");
        let message = error.to_string();
        assert!(
            message.contains("意图解析失败"),
            "错误信息应说明解析失败: {message}"
        );
    }

    #[tokio::test]
    async fn provider_error_is_returned_without_retry() {
        let provider = SeqProvider::new(&["unused"]).with_error("401 未授权");
        let error = parse_intent("修复 a", "rust", &provider).await.unwrap_err();
        assert!(matches!(error, XaimError::Model(ref m) if m == "401 未授权"));
        assert_eq!(provider.call_count(), 1, "传输层错误不应重试");
    }

    #[tokio::test]
    async fn retry_attempt_appends_correction_hint() {
        let provider = SeqProvider::new(&[
            "完全不是 JSON 的输出",
            r#"{"action":"explain","target":"","description":"d","constraints":[],"verify":[]}"#,
        ]);
        let intent = parse_intent("解释", "rust", &provider).await.unwrap();
        assert_eq!(intent.action, "explain");

        let first = provider.requested_messages(0);
        assert_eq!(first.len(), 2, "首次请求应为 system + user 两条消息");
        assert_eq!(first[0].content, prompts::intent::SYSTEM);
        assert!(first[1].content.contains("解释"), "用户任务应进入提示词");

        let second = provider.requested_messages(1);
        let last = second.last().unwrap();
        assert_eq!(last.role, "user");
        assert!(
            last.content.contains("只输出一个 JSON 对象"),
            "重试请求应附带纠正提示: {}",
            last.content
        );
    }

    #[test]
    fn actions_enum_matches_prompt_contract() {
        assert_eq!(ACTIONS.len(), 8);
        let expected = [
            "fix_bug",
            "add_feature",
            "add_test",
            "refactor",
            "review",
            "explain",
            "upgrade",
            "cross_file",
        ];
        assert_eq!(ACTIONS, expected);
        for action in ACTIONS {
            assert!(
                prompts::intent::SYSTEM.contains(action),
                "SYSTEM 提示词应包含枚举值 {action}"
            );
        }
        for action in STEP_ACTIONS {
            assert!(
                prompts::plan::SYSTEM.contains(action),
                "plan SYSTEM 提示词应包含步骤枚举值 {action}"
            );
        }
    }

    #[test]
    fn intent_roundtrips_through_json() {
        let original = make_intent(
            "fix_bug",
            "login.ts",
            "修复空指针",
            &["保持接口兼容"],
            &["type", "test"],
        );
        let json = serde_json::to_string(&original).unwrap();
        let parsed: Intent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["action"], "fix_bug");
        assert_eq!(value["target"], "login.ts");
    }

    #[test]
    fn extract_json_object_finds_braces() {
        assert_eq!(
            extract_json_object("前缀 {\"a\":1} 后缀"),
            Some("{\"a\":1}")
        );
        assert_eq!(
            extract_json_object("{\"a\":{\"b\":2}}"),
            Some("{\"a\":{\"b\":2}}")
        );
        assert_eq!(extract_json_object("没有大括号"), None);
    }

    // ---- 计划生成 ----

    fn step(action: &str, file: &str, reason: &str) -> PlanStep {
        PlanStep {
            action: action.to_string(),
            file: file.to_string(),
            reason: reason.to_string(),
        }
    }

    fn plan(steps: Vec<PlanStep>, files: &[&str], risks: &[&str]) -> Plan {
        Plan {
            steps,
            files_to_modify: files.iter().map(|s| s.to_string()).collect(),
            risks: risks.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn context(files: &[(&str, &str)]) -> Context {
        Context {
            files: files
                .iter()
                .map(|(path, content)| crate::search::context::ContextFile {
                    path: path.to_string(),
                    content: content.to_string(),
                    reason: String::new(),
                    tokens: 0,
                })
                .collect(),
            total_tokens: 0,
        }
    }

    /// 计划样例：任务 + 上下文文件（路径, 片段） -> 模拟模型输出 -> 期望计划。
    type PlanSample = (
        &'static str,
        Vec<(&'static str, &'static str)>,
        &'static str,
        Plan,
    );

    /// 覆盖 read/modify/test 步骤、多文件修改、只读计划、snake_case 键名、
    /// 多余字段容忍与紧凑/排版两种 JSON 形态。
    fn plan_samples() -> Vec<PlanSample> {
        vec![
            (
                "修复登录空指针",
                vec![(
                    "src/auth/login.ts",
                    "export function login(user) { return user.name; }",
                )],
                r#"{
  "steps": [
    {"action": "read", "file": "src/auth/login.ts", "reason": "查看登录逻辑"},
    {"action": "modify", "file": "src/auth/login.ts", "reason": "添加空值检查"},
    {"action": "test", "file": "pnpm test auth", "reason": "验证修改"}
  ],
  "files_to_modify": ["src/auth/login.ts"],
  "risks": ["可能影响其他调用方"]
}"#,
                plan(
                    vec![
                        step("read", "src/auth/login.ts", "查看登录逻辑"),
                        step("modify", "src/auth/login.ts", "添加空值检查"),
                        step("test", "pnpm test auth", "验证修改"),
                    ],
                    &["src/auth/login.ts"],
                    &["可能影响其他调用方"],
                ),
            ),
            (
                "把所有 fetch 调用换成 axios",
                vec![
                    ("src/api/a.ts", "fetch('/x')"),
                    ("src/api/b.ts", "fetch('/y')"),
                ],
                r#"{"steps":[{"action":"read","file":"src/api/a.ts","reason":"确认调用方式"},{"action":"modify","file":"src/api/a.ts","reason":"替换为 axios"},{"action":"modify","file":"src/api/b.ts","reason":"替换为 axios"},{"action":"test","file":"pnpm test","reason":"验证修改"}],"files_to_modify":["src/api/a.ts","src/api/b.ts"],"risks":["影响所有网络请求调用方"]}"#,
                plan(
                    vec![
                        step("read", "src/api/a.ts", "确认调用方式"),
                        step("modify", "src/api/a.ts", "替换为 axios"),
                        step("modify", "src/api/b.ts", "替换为 axios"),
                        step("test", "pnpm test", "验证修改"),
                    ],
                    &["src/api/a.ts", "src/api/b.ts"],
                    &["影响所有网络请求调用方"],
                ),
            ),
            (
                "review payment 模块的错误处理",
                vec![("src/payment/index.ts", "export function pay() {}")],
                r#"{"steps":[{"action":"read","file":"src/payment/index.ts","reason":"通读支付逻辑"},{"action":"read","file":"src/payment/error.ts","reason":"查看错误定义"}],"files_to_modify":[],"risks":[]}"#,
                plan(
                    vec![
                        step("read", "src/payment/index.ts", "通读支付逻辑"),
                        step("read", "src/payment/error.ts", "查看错误定义"),
                    ],
                    &[],
                    &[],
                ),
            ),
            (
                "跑一遍全部测试",
                vec![],
                r#"{"steps":[{"action":"test","file":"pnpm test","reason":"确认当前测试全绿"}],"files_to_modify":[],"risks":["无代码改动"]}"#,
                plan(
                    vec![step("test", "pnpm test", "确认当前测试全绿")],
                    &[],
                    &["无代码改动"],
                ),
            ),
            (
                "修复越界 panic",
                vec![],
                r#"{"steps":[{"action":"read","file":"src/x.ts","reason":"查看代码"},{"action":"modify","file":"src/x.ts","reason":"加边界检查"},{"action":"test","file":"pnpm test","reason":"验证"}],"files_to_modify":["src/x.ts"],"risks":[]}"#,
                plan(
                    vec![
                        step("read", "src/x.ts", "查看代码"),
                        step("modify", "src/x.ts", "加边界检查"),
                        step("test", "pnpm test", "验证"),
                    ],
                    &["src/x.ts"],
                    &[],
                ),
            ),
            (
                "升级依赖",
                vec![("package.json", "{}")],
                r#"{"steps":[{"action":"modify","file":"package.json","reason":"升级版本号"},{"action":"test","file":"pnpm test","reason":"验证兼容"}],"files_to_modify":["package.json"],"risks":["可能引入破坏性变更","需要重新构建"]}"#,
                plan(
                    vec![
                        step("modify", "package.json", "升级版本号"),
                        step("test", "pnpm test", "验证兼容"),
                    ],
                    &["package.json"],
                    &["可能引入破坏性变更", "需要重新构建"],
                ),
            ),
            (
                "给 auth 补测试",
                vec![("src/auth/index.ts", "export login()")],
                // 模型偶发多余字段（command）：应被忽略，file 承载命令
                r#"{"steps":[{"action":"test","file":"pnpm test auth","command":"pnpm test auth","reason":"验证用例可跑"},{"action":"modify","file":"src/auth/index.ts","reason":"补分支覆盖"}],"files_to_modify":["src/auth/index.ts"],"risks":[]}"#,
                plan(
                    vec![
                        step("test", "pnpm test auth", "验证用例可跑"),
                        step("modify", "src/auth/index.ts", "补分支覆盖"),
                    ],
                    &["src/auth/index.ts"],
                    &[],
                ),
            ),
        ]
    }

    #[tokio::test]
    async fn parses_plan_samples_end_to_end() {
        for (index, (task, files, response, expected)) in plan_samples().into_iter().enumerate() {
            let provider = MockProvider::new(response);
            let parsed = generate_plan(task, &context(&files), &provider)
                .await
                .unwrap_or_else(|error| {
                    panic!("计划样例 {}（{task}）解析失败: {error}", index + 1)
                });
            assert_eq!(parsed, expected, "计划样例 {}（{task}）不符", index + 1);
        }
    }

    /// 「计划可读」验收：每个样例的步骤均有合法 action、非空 file 与 reason，
    /// modify 步骤存在时 files_to_modify 必须列出文件。
    #[test]
    fn sample_plans_are_readable() {
        for (task, _, _, plan) in plan_samples() {
            assert!(!plan.steps.is_empty(), "{task} 的计划必须有步骤");
            for s in &plan.steps {
                assert!(
                    STEP_ACTIONS.contains(&s.action.as_str()),
                    "{task} 步骤 action 非法: {}",
                    s.action
                );
                assert!(!s.file.is_empty(), "{task} 步骤缺 file: {}", s.action);
                assert!(!s.reason.is_empty(), "{task} 步骤缺 reason: {}", s.action);
            }
            if plan.steps.iter().any(|s| s.action == "modify") {
                assert!(
                    !plan.files_to_modify.is_empty(),
                    "{task} 有修改步骤但未列出要修改的文件"
                );
            }
        }
    }

    #[tokio::test]
    async fn markdown_fenced_plan_is_parsed() {
        let response = "```json\n{\"steps\":[{\"action\":\"test\",\"file\":\"pnpm test\",\"reason\":\"验证\"}],\"files_to_modify\":[],\"risks\":[]}\n```";
        let provider = MockProvider::new(response);
        let parsed = generate_plan("跑测试", &context(&[]), &provider)
            .await
            .unwrap();
        assert_eq!(parsed.steps.len(), 1);
        assert_eq!(parsed.steps[0].action, "test");
    }

    #[tokio::test]
    async fn plan_without_optional_fields_defaults_to_empty() {
        let provider = MockProvider::new(
            r#"{"steps": [{"action": "read", "file": "src/a.ts", "reason": "查看"}]}"#,
        );
        let parsed = generate_plan("看看 a.ts", &context(&[]), &provider)
            .await
            .unwrap();
        assert_eq!(parsed.files_to_modify, Vec::<String>::new());
        assert_eq!(parsed.risks, Vec::<String>::new());
    }

    #[tokio::test]
    async fn plan_without_steps_is_rejected_then_retry_succeeds() {
        let provider = SeqProvider::new(&[
            r#"{"files_to_modify": [], "risks": []}"#,
            r#"{"steps":[{"action":"read","file":"src/a.ts","reason":"查看"}],"files_to_modify":[],"risks":[]}"#,
        ]);
        let parsed = generate_plan("看看 a.ts", &context(&[]), &provider)
            .await
            .unwrap();
        assert_eq!(parsed.steps.len(), 1);
        assert_eq!(provider.call_count(), 2, "缺 steps 应触发一次重试");
    }

    #[tokio::test]
    async fn modify_steps_without_files_to_modify_are_rejected() {
        let provider = SeqProvider::new(&[
            r#"{"steps":[{"action":"modify","file":"src/a.ts","reason":"修改"}],"files_to_modify":[],"risks":[]}"#,
            r#"{"steps":[{"action":"modify","file":"src/a.ts","reason":"修改"}],"files_to_modify":["src/a.ts"],"risks":[]}"#,
        ]);
        let parsed = generate_plan("改 a.ts", &context(&[]), &provider)
            .await
            .unwrap();
        assert_eq!(parsed.files_to_modify, vec!["src/a.ts".to_string()]);
        assert_eq!(provider.call_count(), 2, "modify 步骤但未列文件应触发重试");
    }

    #[tokio::test]
    async fn step_with_unknown_action_or_missing_file_is_rejected() {
        let provider = SeqProvider::new(&[
            r#"{"steps":[{"action":"edit","file":"src/a.ts","reason":"改"}],"files_to_modify":["src/a.ts"],"risks":[]}"#,
            r#"{"steps":[{"action":"modify","file":"","reason":"改"}],"files_to_modify":["src/a.ts"],"risks":[]}"#,
            r#"{"steps":[{"action":"modify","file":"src/a.ts","reason":"改"}],"files_to_modify":["src/a.ts"],"risks":[]}"#,
        ]);
        let parsed = generate_plan("改 a.ts", &context(&[]), &provider)
            .await
            .unwrap();
        assert_eq!(parsed.steps[0].action, "modify");
        assert_eq!(
            provider.call_count(),
            3,
            "非法 action 与空 file 应各触发一次重试"
        );
    }

    #[tokio::test]
    async fn malformed_plan_exhausts_two_retries_then_fails() {
        let provider = SeqProvider::new(&["这不是 JSON"]);
        let error = generate_plan("改 a.ts", &context(&[]), &provider)
            .await
            .unwrap_err();
        assert!(matches!(error, XaimError::Model(_)), "实际错误: {error:?}");
        assert_eq!(provider.call_count(), 3, "首次 + 重试 2 次，共 3 次调用");
        let message = error.to_string();
        assert!(
            message.contains("计划生成失败"),
            "错误信息应说明计划生成失败: {message}"
        );
    }

    #[tokio::test]
    async fn plan_provider_error_is_returned_without_retry() {
        let provider = SeqProvider::new(&["unused"]).with_error("429 限流");
        let error = generate_plan("改 a.ts", &context(&[]), &provider)
            .await
            .unwrap_err();
        assert!(matches!(error, XaimError::Model(ref m) if m == "429 限流"));
        assert_eq!(provider.call_count(), 1, "传输层错误不应重试");
    }

    #[tokio::test]
    async fn generate_plan_prompts_contain_task_context_and_routes_strong() {
        let provider = SeqProvider::new(&[
            r#"{"steps":[{"action":"test","file":"pnpm test","reason":"验证"}],"files_to_modify":[],"risks":[]}"#,
        ]);
        generate_plan(
            "修复登录",
            &context(&[("src/auth/login.ts", "export login()")]),
            &provider,
        )
        .await
        .unwrap();

        // 计划生成属生成类任务，默认路由 Strong 档模型
        assert_eq!(provider.requested_model(0), "deepseek-reasoner");

        let messages = provider.requested_messages(0);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[0].content, prompts::plan::SYSTEM);
        let user = &messages[1].content;
        assert!(user.contains("修复登录"), "应包含任务: {user}");
        assert!(
            user.contains("src/auth/login.ts"),
            "应包含上下文路径: {user}"
        );
        assert!(user.contains("export login()"), "应包含上下文片段: {user}");
    }

    #[test]
    fn plan_roundtrips_through_json_with_both_key_styles() {
        let original = plan(
            vec![step("modify", "src/a.ts", "修改")],
            &["src/a.ts"],
            &["低风险"],
        );

        // IPC 契约：camelCase 序列化，可往返
        let json = serde_json::to_string(&original).unwrap();
        assert!(
            json.contains("filesToModify"),
            "IPC 序列化应为 camelCase: {json}"
        );
        let parsed: Plan = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);

        // 模型契约：snake_case 的 files_to_modify 经 RawPlan 的 alias 解析
        let model_json = r#"{"steps":[{"action":"modify","file":"src/a.ts","reason":"修改"}],"files_to_modify":["src/a.ts"],"risks":["低风险"]}"#;
        let from_model = parse_plan_json(model_json).unwrap();
        assert_eq!(from_model, original);
    }

    // ---- 候选 Patch 生成 ----

    /// 构造最小合法 unified diff（新增一行）。
    fn diff_for(path: &str, added: &str) -> String {
        format!("--- a/{path}\n+++ b/{path}\n@@ -1,1 +1,2 @@\n const a = 1;\n+{added}\n")
    }

    /// 两文件联动的 diff 文本。
    fn multi_file_diff() -> String {
        format!(
            "{}{}",
            diff_for("src/a.ts", "const b = 2;"),
            diff_for("src/b.ts", "const d = 4;")
        )
    }

    fn simple_plan() -> Plan {
        plan(
            vec![
                step("read", "src/auth/login.ts", "查看登录逻辑"),
                step("modify", "src/auth/login.ts", "添加空值检查"),
                step("test", "pnpm test auth", "验证修改"),
            ],
            &["src/auth/login.ts"],
            &["可能影响其他调用方"],
        )
    }

    fn temperature_equals(actual: f32, expected: f32) -> bool {
        (actual - expected).abs() < 1e-6
    }

    #[tokio::test]
    async fn generates_two_distinct_candidates_in_parallel() {
        let provider = SeqProvider::of(vec![
            diff_for("src/auth/login.ts", "if (!user) throw new Error(\"null\");"),
            diff_for("src/auth/login.ts", "user ??= defaultUser;"),
        ]);
        let candidates =
            generate_candidates("修复登录", &simple_plan(), &context(&[]), &provider, 2)
                .await
                .unwrap();

        assert_eq!(candidates.len(), 2, "两个不同补丁应保留两个候选");
        assert_eq!(candidates[0].file_path, "src/auth/login.ts");
        assert_eq!(candidates[0].score, None, "解析阶段不评分");
        assert_eq!(provider.call_count(), 2, "应并行调用 2 次");
        assert!(
            temperature_equals(provider.requested_temperature(0), 0.1)
                && temperature_equals(provider.requested_temperature(1), 0.2),
            "温度应按 0.1/0.2 递增: {}, {}",
            provider.requested_temperature(0),
            provider.requested_temperature(1)
        );
    }

    #[tokio::test]
    async fn generates_three_candidates_with_increasing_temperature() {
        let provider = SeqProvider::of(vec![
            diff_for("src/a.ts", "方案一"),
            diff_for("src/a.ts", "方案二"),
            diff_for("src/a.ts", "方案三"),
        ]);
        let candidates = generate_candidates("改 a", &simple_plan(), &context(&[]), &provider, 3)
            .await
            .unwrap();

        assert_eq!(candidates.len(), 3, "三个不同补丁应保留三个候选");
        for (index, expected) in [0.1, 0.2, 0.3].iter().enumerate() {
            assert!(
                temperature_equals(provider.requested_temperature(index), *expected),
                "第 {} 个候选温度应为 {expected}，实际 {}",
                index + 1,
                provider.requested_temperature(index)
            );
        }
    }

    #[tokio::test]
    async fn duplicate_candidates_are_deduped() {
        let same_diff = diff_for("src/a.ts", "const b = 2;");
        let provider = SeqProvider::new(&[&same_diff, &same_diff, &same_diff]);
        let candidates = generate_candidates("改 a", &simple_plan(), &context(&[]), &provider, 3)
            .await
            .unwrap();

        assert_eq!(candidates.len(), 1, "完全相同的补丁应去重为一个候选");
        assert_eq!(provider.call_count(), 3, "去重前仍应完成全部 3 次调用");
    }

    #[tokio::test]
    async fn multi_file_patch_yields_multiple_candidates() {
        let provider = SeqProvider::of(vec![multi_file_diff()]);
        let candidates =
            generate_candidates("改 a 和 b", &simple_plan(), &context(&[]), &provider, 1)
                .await
                .unwrap();

        assert_eq!(candidates.len(), 2, "单次多文件 diff 应切成两个候选");
        assert_eq!(candidates[0].file_path, "src/a.ts");
        assert_eq!(candidates[1].file_path, "src/b.ts");
    }

    #[tokio::test]
    async fn invalid_output_does_not_affect_other_candidates() {
        let provider = SeqProvider::of(vec![
            "这段输出里没有 diff".to_string(),
            diff_for("src/a.ts", "方案二"),
            diff_for("src/a.ts", "方案三"),
        ]);
        let candidates = generate_candidates("改 a", &simple_plan(), &context(&[]), &provider, 3)
            .await
            .unwrap();

        assert_eq!(candidates.len(), 2, "无效输出只淘汰该候选");
        assert_eq!(provider.call_count(), 3);
    }

    #[tokio::test]
    async fn transport_failure_does_not_affect_other_candidates() {
        let provider = SeqProvider::of(vec![
            diff_for("src/a.ts", "方案二"),
            diff_for("src/a.ts", "方案三"),
        ])
        .with_fail_first(1);
        let candidates = generate_candidates("改 a", &simple_plan(), &context(&[]), &provider, 3)
            .await
            .unwrap();

        assert_eq!(candidates.len(), 2, "传输失败的候选应被跳过");
        assert_eq!(provider.call_count(), 3);
    }

    #[tokio::test]
    async fn all_candidates_failing_returns_error() {
        let provider = SeqProvider::new(&["没有 diff", "还是没有 diff"]);
        let error = generate_candidates("改 a", &simple_plan(), &context(&[]), &provider, 2)
            .await
            .unwrap_err();

        assert!(matches!(error, XaimError::Model(_)), "实际错误: {error:?}");
        assert_eq!(provider.call_count(), 3, "候选失败后只追加一次格式纠正");
        let message = error.to_string();
        assert!(
            message.contains("候选生成失败"),
            "错误信息应说明全部候选失败: {message}"
        );
    }

    #[tokio::test]
    async fn zero_candidates_requested_returns_empty_without_calls() {
        let provider = SeqProvider::new(&["unused"]);
        let candidates = generate_candidates("改 a", &simple_plan(), &context(&[]), &provider, 0)
            .await
            .unwrap();
        assert!(candidates.is_empty());
        assert_eq!(provider.call_count(), 0);
    }

    #[tokio::test]
    async fn new_html_json_is_converted_to_patch_for_a_confirmed_absent_path() {
        let root = tempfile::tempdir().unwrap();
        let html = "<!doctype html>\n<html><style>svg { color: black; }</style><svg><!-- --- marker --></svg></html>";
        let response =
            serde_json::json!({"files":[{"path":"web/index.html","content":html}]}).to_string();
        let provider = SeqProvider::new(&[&response]);
        let plan = plan(
            vec![step("modify", "web/index.html", "create")],
            &["web/index.html"],
            &[],
        );
        let candidates = generate_candidates_in_workspace(
            "创建 HTML",
            &plan,
            &context(&[("package.json", "{\"type\":\"module\"}")]),
            "selected",
            &provider,
            1,
            root.path(),
        )
        .await
        .unwrap();
        assert!(provider.requested_messages(0)[0]
            .content
            .contains("文件生成器"));
        assert!(provider.requested_messages(0)[1]
            .content
            .contains("{\"type\":\"module\"}"));
        assert!(candidates[0].diff.starts_with("--- /dev/null\n"));
        crate::tools::patch::apply_diff(&candidates[0].diff, root.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("web/index.html")).unwrap(),
            html
        );
    }

    #[test]
    fn generated_file_json_rejects_unplanned_existing_duplicate_and_empty_content() {
        let allowed = vec!["index.html".to_string()];
        for output in [
            r#"{"files":[{"path":"../outside.html","content":"hello"}]}"#,
            r#"{"files":[{"path":"existing.html","content":"hello"}]}"#,
            r#"{"files":[{"path":"index.html","content":"one"},{"path":"index.html","content":"two"}]}"#,
            r#"{"files":[{"path":"index.html","content":""}]}"#,
            r#"{"files":[]}"#,
        ] {
            assert!(parse_candidate_response(output, &allowed).is_err());
        }
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), "existing").unwrap();
        let plan = plan(
            vec![step("modify", "index.html", "edit")],
            &["index.html"],
            &[],
        );
        assert!(planned_new_paths(&plan, root.path()).unwrap().is_empty());
        assert!(parse_candidate_response(
            r#"{"files":[{"path":"index.html","content":"replace"}]}"#,
            &[]
        )
        .is_err());
    }

    #[tokio::test]
    async fn raw_html_is_corrected_to_new_file_diff() {
        let diff = "--- /dev/null\n+++ b/web/index.html\n@@ -0,0 +1,2 @@\n+<!doctype html>\n+<html></html>\n";
        let provider = SeqProvider::new(&["<!doctype html><html></html>", diff]);
        let candidates = generate_candidates_with_model(
            "创建 HTML",
            &simple_plan(),
            &context(&[]),
            "selected-model",
            &provider,
            1,
        )
        .await
        .unwrap();
        assert_eq!(candidates[0].file_path, "web/index.html");
        assert_eq!(provider.call_count(), 2);
        assert_eq!(provider.requested_model(1), "selected-model");
        let messages = provider.requested_messages(1);
        assert_eq!(messages[2].role, "assistant");
        assert!(messages.last().unwrap().content.contains("--- /dev/null"));
        let root = tempfile::tempdir().unwrap();
        crate::tools::patch::apply_diff(&candidates[0].diff, root.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("web/index.html")).unwrap(),
            "<!doctype html>\n<html></html>\n"
        );
    }

    #[tokio::test]
    async fn malformed_hunk_is_corrected_and_retry_is_bounded() {
        let invalid = "--- /dev/null\n+++ b/index.html\n@@ -0,0 +1,9 @@\n+<html></html>\n";
        let provider = SeqProvider::new(&[invalid]);
        let error = generate_candidates("创建 HTML", &simple_plan(), &context(&[]), &provider, 1)
            .await
            .unwrap_err();
        assert_eq!(provider.call_count(), 2);
        assert!(error.to_string().contains("格式纠正后仍无有效补丁"));
    }

    #[tokio::test]
    async fn transport_errors_do_not_trigger_format_correction() {
        let provider = SeqProvider::new(&["unused"]).with_error("offline");
        assert!(
            generate_candidates("创建 HTML", &simple_plan(), &context(&[]), &provider, 2)
                .await
                .is_err()
        );
        assert_eq!(provider.call_count(), 2);
    }

    #[tokio::test]
    async fn candidates_prompt_contains_task_plan_context_and_routes_strong() {
        let provider = SeqProvider::of(vec![diff_for("src/auth/login.ts", "fix")]);
        generate_candidates(
            "修复登录",
            &simple_plan(),
            &context(&[("src/auth/login.ts", "export login()")]),
            &provider,
            2,
        )
        .await
        .unwrap();

        // 补丁生成属生成类任务，默认路由 Strong 档模型
        assert_eq!(provider.requested_model(0), "deepseek-reasoner");

        let messages = provider.requested_messages(0);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, prompts::modify::SYSTEM);
        let user = &messages[1].content;
        assert!(user.contains("修复登录"), "应包含任务: {user}");
        assert!(
            user.contains("[read] src/auth/login.ts"),
            "应包含计划步骤: {user}"
        );
        assert!(user.contains("添加空值检查"), "应包含步骤理由: {user}");
        assert!(
            user.contains("待修改文件：src/auth/login.ts"),
            "应包含文件清单: {user}"
        );
        assert!(
            user.contains("风险：可能影响其他调用方"),
            "应包含风险: {user}"
        );
        assert!(
            user.contains("=== src/auth/login.ts ==="),
            "应包含上下文: {user}"
        );
        assert!(user.contains("export login()"), "应包含上下文片段: {user}");
    }

    #[test]
    fn candidate_temperature_increments_then_caps() {
        let expected = [0.1, 0.2, 0.3, 0.3, 0.3];
        for (index, expected) in expected.iter().enumerate() {
            assert!(
                temperature_equals(candidate_temperature(index), *expected),
                "candidate_temperature({index}) 应为 {expected}"
            );
        }
    }

    #[test]
    fn format_plan_is_readable() {
        let text = format_plan(&simple_plan());
        assert!(
            text.contains("1. [read] src/auth/login.ts — 查看登录逻辑"),
            "{text}"
        );
        assert!(
            text.contains("2. [modify] src/auth/login.ts — 添加空值检查"),
            "{text}"
        );
        assert!(
            text.contains("3. [test] pnpm test auth — 验证修改"),
            "{text}"
        );
        assert!(text.contains("待修改文件：src/auth/login.ts"), "{text}");
        assert!(text.contains("风险：可能影响其他调用方"), "{text}");
    }
}
