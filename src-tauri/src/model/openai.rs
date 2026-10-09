//! OpenAI 兼容提供方：实现 Chat Completions 协议（`POST {base_url}/chat/completions`）。
//!
//! 同一套实现覆盖所有兼容该协议的服务，仅 base_url 不同：
//! OpenAI 官方（`https://api.openai.com/v1`）、
//! DeepSeek（`https://api.deepseek.com/v1`）、
//! Ollama 本地模型（`http://localhost:11434/v1`）等。

use std::pin::Pin;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use super::provider::{
    CompletionRequest, CompletionResponse, ModelProvider, RequestObserver, ToolMessage,
    ToolRequest, ToolResponse, Usage,
};
use crate::errors::{Result, XaimError};

/// 单次请求超时：非流式补全的整体上限；流式请求用它限制读空闲
/// （120s 内未收到任何字节即断开），整体时长不设上限以免截断长生成。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const TOOL_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
/// 失败后的重试次数（不含首次请求）。
const MAX_RETRIES: u32 = 2;
/// 指数退避基数：第 n 次重试前等待 `BACKOFF_BASE_MS * 2^(n-1)` 毫秒。
const BACKOFF_BASE_MS: u64 = 500;
/// OpenAI 协议的流结束哨兵。
const DONE_SENTINEL: &str = "[DONE]";

/// OpenAI 兼容提供方。
pub struct OpenAiProvider {
    provider_name: &'static str,
    base_url: String,
    api_key: String,
    client: reqwest::Client,
    tool_client: reqwest::Client,
    reasoning_effort: Option<String>,
    reasoning_model: bool,
    deepseek: bool,
    qwen: bool,
    thinking: Option<bool>,
    output_limit: Option<u32>,
    temperature: Option<f32>,
}

impl OpenAiProvider {
    pub fn new(base_url: String, api_key: String) -> Self {
        // 不设客户端级总超时：流式响应的时长由内容决定；空闲保护交给 read_timeout。
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(REQUEST_TIMEOUT)
            .build()
            .expect("构建 reqwest Client 失败");
        let tool_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(TOOL_REQUEST_TIMEOUT)
            .build()
            .expect("构建工具请求 Client 失败");
        Self {
            provider_name: "openai",
            base_url: normalize_base_url(&base_url),
            api_key: normalize_api_key(&api_key),
            client,
            tool_client,
            reasoning_effort: None,
            reasoning_model: false,
            deepseek: false,
            qwen: false,
            thinking: None,
            output_limit: None,
            temperature: None,
        }
    }

    pub fn with_reasoning_effort(mut self, effort: Option<String>) -> Self {
        self.reasoning_effort = effort;
        self
    }

    pub fn with_reasoning_model(mut self, reasoning: bool) -> Self {
        self.reasoning_model = reasoning;
        self
    }

    pub fn with_deepseek_defaults(mut self) -> Self {
        self.deepseek = true;
        self.provider_name = "deepseek";
        self
    }

    pub fn with_output_limit(mut self, limit: Option<u32>) -> Self {
        self.output_limit = limit;
        self
    }

    pub fn with_temperature(mut self, temperature: Option<f32>) -> Self {
        self.temperature = temperature;
        self
    }

    pub fn with_thinking(mut self, thinking: Option<bool>) -> Self {
        self.thinking = thinking;
        self
    }

    pub fn with_qwen_defaults(mut self) -> Self {
        self.qwen = true;
        self.provider_name = "qwen";
        self
    }

    pub fn with_provider_name(mut self, name: &'static str) -> Self {
        self.provider_name = name;
        self
    }

    fn tool_payload(&self, req: &ToolRequest) -> Result<serde_json::Value> {
        let plain = CompletionRequest::new(&req.model, vec![]);
        let mut payload = serde_json::to_value(self.payload(&plain, false))?;
        payload["messages"] = serde_json::to_value(&req.messages)?;
        payload["tools"] = serde_json::to_value(&req.tools)?;
        let limit = self
            .output_limit
            .map_or(req.max_tokens, |saved| saved.min(req.max_tokens));
        let key = if !self.deepseek
            && !self.qwen
            && (self.reasoning_model || self.reasoning_effort.is_some())
        {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        let object = payload.as_object_mut().unwrap();
        object.remove("max_tokens");
        object.remove("max_completion_tokens");
        object.insert(key.into(), serde_json::json!(limit));
        let effort = self.reasoning_effort.as_deref();
        if !self.deepseek && !self.qwen {
            if let Some(effort) = effort {
                object.insert("reasoning_effort".into(), serde_json::json!(effort));
            }
        }
        if self.deepseek || self.qwen {
            object.remove("reasoning_effort");
            if let Some(thinking) = self.thinking {
                if self.deepseek {
                    object.insert(
                        "thinking".into(),
                        serde_json::json!({"type":if thinking { "enabled" } else { "disabled" }}),
                    );
                } else {
                    object.insert("enable_thinking".into(), serde_json::json!(thinking));
                }
            }
            if self.deepseek && self.thinking != Some(false) {
                if let Some(effort) = effort {
                    object.insert("reasoning_effort".into(), serde_json::json!(effort));
                }
            }
        }
        Ok(payload)
    }

    fn payload<'a>(&'a self, req: &'a CompletionRequest, stream: bool) -> WireRequest<'a> {
        let mut payload = wire_request(req, stream);
        payload.temperature = Some(self.temperature.unwrap_or(req.temperature));
        let deepseek_thinking = self.deepseek
            && matches!(
                req.model.as_str(),
                "deepseek-flash" | "deepseek-v4-pro" | "deepseek-reasoner"
            );
        let budget = self.output_limit.map_or_else(
            || {
                if deepseek_thinking && self.thinking != Some(false) {
                    req.max_tokens.max(32_768)
                } else {
                    req.max_tokens
                }
            },
            |limit| limit.min(req.max_tokens),
        );
        if !self.deepseek && !self.qwen && (self.reasoning_model || self.reasoning_effort.is_some())
        {
            payload.reasoning_effort = self.reasoning_effort.as_deref();
            payload.temperature = None;
            payload.max_tokens = None;
            payload.max_completion_tokens = Some(budget);
        } else {
            payload.max_tokens = Some(budget);
            if deepseek_thinking && self.thinking != Some(false) {
                // DeepSeek uses max_tokens for thinking + answer; temperature is ignored.
                payload.temperature = None;
            }
        }
        if self.deepseek {
            payload.thinking = self.thinking.map(
                |enabled| serde_json::json!({"type":if enabled { "enabled" } else { "disabled" }}),
            );
            if self.thinking != Some(false) {
                payload.reasoning_effort = self.reasoning_effort.as_deref();
            }
        } else if self.qwen {
            payload.enable_thinking = self.thinking;
        }
        payload
    }

    /// 构造时使用的 API 地址。
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 发送请求并在可重试的失败（网络错误、408/429/5xx）上指数退避重试。
    /// 只重试到拿到成功响应为止，响应体的消费不在重试范围内。
    /// `timeout` 为整体超时：文本补全传 [`REQUEST_TIMEOUT`]，工具补全传
    /// [`TOOL_REQUEST_TIMEOUT`]，流式传 `None`
    /// （流式只受客户端 read_timeout 的空闲保护）。
    async fn send_with_retries<T: Serialize + Sync>(
        &self,
        payload: &T,
        timeout: Option<Duration>,
        deadline: Option<Instant>,
        observer: Option<&RequestObserver<'_>>,
    ) -> Result<reqwest::Response> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut attempt = 0u32;
        loop {
            let remaining = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            if remaining.is_some_and(|d| d.is_zero()) {
                return Err(XaimError::Timeout("模型请求达到任务截止时间".into()));
            }
            let mut observation = RequestObservation::new(observer, "headers", attempt + 1);
            observation.emit(serde_json::json!({"stage":"attemptStarted","attempt":attempt+1}));
            let client = if timeout == Some(TOOL_REQUEST_TIMEOUT) {
                &self.tool_client
            } else {
                &self.client
            };
            let mut request = client.post(&url).bearer_auth(&self.api_key).json(payload);
            if let Some(timeout) = timeout {
                request = request.timeout(remaining.map_or(timeout, |r| r.min(timeout)));
            }
            let send = request.send().await;

            match send {
                Ok(resp) if resp.status().is_success() => {
                    observation.finish(serde_json::json!({"stage":"headers","attempt":attempt+1,
                        "status":resp.status().as_u16(),"durationMs":observation.elapsed_ms(),"requestId":request_id(&resp)}));
                    return Ok(resp);
                }
                Ok(resp) => {
                    let status = resp.status();
                    let retry_after = resp
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(retry_after_delay);
                    observation.emit(serde_json::json!({"stage":"headers","attempt":attempt+1,
                        "status":status.as_u16(),"durationMs":observation.elapsed_ms(),"requestId":request_id(&resp)}));
                    observation.stage = "error_body";
                    let body = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "(读取响应体失败)".to_string());
                    observation.finish(serde_json::json!({"stage":"httpError","attempt":attempt+1,
                        "status":status.as_u16(),"durationMs":observation.elapsed_ms()}));
                    if attempt < MAX_RETRIES && is_retryable_status(status) {
                        attempt += 1;
                        tracing::warn!(attempt, %status, "请求失败，稍后重试");
                        let delay = retry_after.unwrap_or_else(|| backoff_delay(attempt));
                        retry_wait(delay, deadline, observer, attempt).await?;
                        continue;
                    }
                    if status == reqwest::StatusCode::UNAUTHORIZED {
                        return Err(XaimError::Model(format!(
                            "HTTP 401 Unauthorized: {body}; 请检查 API Key 是否为裸 token（不要填写 Bearer 前缀）、Base URL 是否为 OpenAI 兼容根地址（通常以 /v1 结尾），以及该 token 是否有权访问模型"
                        )));
                    }
                    return Err(XaimError::Model(format!("HTTP {status}: {body}")));
                }
                Err(err) => {
                    observation.finish(serde_json::json!({"stage":"transportError","attempt":attempt+1,
                        "durationMs":observation.elapsed_ms(),"connect":err.is_connect(),
                        "timeout":err.is_timeout(),"body":err.is_body(),"request":err.is_request()}));
                    let retryable = err.is_connect() || err.is_body() || err.is_timeout();
                    if attempt < MAX_RETRIES && retryable {
                        attempt += 1;
                        tracing::warn!(attempt, error = %err, "网络错误，稍后重试");
                        retry_wait(backoff_delay(attempt), deadline, observer, attempt).await?;
                        continue;
                    }
                    return Err(XaimError::Http(err));
                }
            }
        }
    }
}

/// Accept either an OpenAI-compatible root (`.../v1`) or a copied endpoint URL.
/// Gateway prefixes such as `/newapi/v1` are preserved.
fn normalize_base_url(value: &str) -> String {
    let mut url = value.trim().trim_end_matches('/').to_string();
    for suffix in ["/chat/completions", "/completions"] {
        if url.ends_with(suffix) {
            url.truncate(url.len() - suffix.len());
            break;
        }
    }
    url.trim_end_matches('/').to_string()
}

/// The request builder owns the `Bearer` scheme, so stored keys are bare tokens.
fn normalize_api_key(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

/// 可重试的状态码：请求超时、限流与服务端临时故障。
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429) || status.is_server_error()
}

fn backoff_delay(attempt: u32) -> Duration {
    let jitter = uuid::Uuid::new_v4().as_u128() as u64 % 251;
    Duration::from_millis((BACKOFF_BASE_MS << (attempt - 1)) + jitter)
}

fn retry_after_delay(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|date| {
            Duration::from_millis(
                (date.timestamp_millis() - chrono::Utc::now().timestamp_millis()).max(0) as u64,
            )
        })
}

fn request_id(response: &reqwest::Response) -> Option<String> {
    ["x-request-id", "request-id"].iter().find_map(|name| {
        response
            .headers()
            .get(*name)
            .and_then(|v| v.to_str().ok())
            .filter(|v| {
                v.len() <= 128
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
            .map(str::to_owned)
    })
}

struct RequestObservation<'a> {
    observer: Option<&'a RequestObserver<'a>>,
    started: Instant,
    stage: &'static str,
    attempt: u32,
    finished: bool,
}

impl<'a> RequestObservation<'a> {
    fn new(observer: Option<&'a RequestObserver<'a>>, stage: &'static str, attempt: u32) -> Self {
        Self {
            observer,
            started: Instant::now(),
            stage,
            attempt,
            finished: false,
        }
    }
    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
    fn emit(&self, data: serde_json::Value) {
        if let Some(observer) = self.observer {
            observer(data);
        }
    }
    fn finish(&mut self, data: serde_json::Value) {
        self.finished = true;
        self.emit(data);
    }
}

impl Drop for RequestObservation<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.emit(
                serde_json::json!({"stage":self.stage,"attempt":self.attempt,
                "cancelled":true,"durationMs":self.elapsed_ms()}),
            );
        }
    }
}

async fn retry_wait(
    delay: Duration,
    deadline: Option<Instant>,
    observer: Option<&RequestObserver<'_>>,
    attempt: u32,
) -> Result<()> {
    if deadline.is_some_and(|d| d.saturating_duration_since(Instant::now()) <= delay) {
        if let Some(observer) = observer {
            observer(
                serde_json::json!({"stage":"retrySkipped","attempt":attempt,"reason":"task_deadline"}),
            );
        }
        return Err(XaimError::Timeout("剩余任务时间不足以重试模型请求".into()));
    }
    let mut observation = RequestObservation::new(observer, "retry_wait", attempt);
    observation.emit(
        serde_json::json!({"stage":"retryScheduled","attempt":attempt,"delayMs":delay.as_millis()}),
    );
    tokio::time::sleep(delay).await;
    observation.finish(serde_json::json!({"stage":"retryWaitComplete","attempt":attempt,"durationMs":observation.elapsed_ms()}));
    Ok(())
}

/// 从一条 SSE data 中提取增量文本；空增量（仅 role 的首块、
/// 纯 usage 收尾块）返回 `Ok(None)`，解析失败时错误里带上原始数据。
fn parse_stream_delta(data: &str) -> Result<Option<String>> {
    let chunk: StreamChunk = serde_json::from_str(data)
        .map_err(|e| XaimError::Model(format!("SSE 数据解析失败: {e}; body: {data}")))?;
    let text = chunk
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.delta.content);
    Ok(text.filter(|t| !t.is_empty()))
}

fn wire_request(req: &CompletionRequest, stream: bool) -> WireRequest<'_> {
    WireRequest {
        model: &req.model,
        messages: req
            .messages
            .iter()
            .map(|m| WireMessage {
                role: &m.role,
                content: &m.content,
            })
            .collect(),
        temperature: Some(req.temperature),
        max_tokens: Some(req.max_tokens),
        max_completion_tokens: None,
        reasoning_effort: None,
        thinking: None,
        enable_thinking: None,
        stream,
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn supports_checkpoint_context(&self) -> bool {
        true
    }

    fn tool_request_metadata(&self, req: &ToolRequest) -> serde_json::Value {
        match self.tool_payload(req) {
            Ok(payload) => serde_json::json!({
                "provider":self.name(),"protocol":"chat_completions","model":payload["model"],
                "endpoint":format!("{}/chat/completions", self.base_url),
                "auth":"bearer",
                "thinking":payload.get("thinking").cloned().or_else(|| payload.get("enable_thinking").cloned()),
                "thinkingSource":if self.thinking.is_some() { "explicit" } else { "provider_default" },
                "reasoningEffort":payload["reasoning_effort"],
                "reasoningEffortSource":if self.reasoning_effort.is_some() { "configured" } else { "provider_default" },
                "maxTokens":payload["max_tokens"],"maxCompletionTokens":payload["max_completion_tokens"],
                "temperature":payload["temperature"],"parameterSource":"wire_payload",
            }),
            Err(error) => {
                serde_json::json!({"provider":self.name(),"model":req.model,"payloadError":error.to_string()})
            }
        }
    }
    fn supports_tools(&self) -> bool {
        true
    }

    async fn complete_tools(&self, req: ToolRequest) -> Result<ToolResponse> {
        self.complete_tools_observed(req, None, &|_| {}).await
    }

    async fn complete_tools_observed(
        &self,
        req: ToolRequest,
        deadline: Option<Instant>,
        observer: &RequestObserver<'_>,
    ) -> Result<ToolResponse> {
        let mut total = RequestObservation::new(Some(observer), "request", 0);
        let outcome = async {
        let payload = self.tool_payload(&req)?;
        tracing::info!(parameters = %self.tool_request_metadata(&req), "发送工具模型请求（脱敏参数）");
        let resp = self
            .send_with_retries(
                &payload,
                Some(TOOL_REQUEST_TIMEOUT),
                deadline,
                Some(observer),
            )
            .await?;
        let mut body_observation = RequestObservation::new(Some(observer), "body", 0);
        let body = match resp.text().await {
            Ok(body) => {
                body_observation.finish(serde_json::json!({"stage":"body","durationMs":body_observation.elapsed_ms(),"bytes":body.len()}));
                body
            }
            Err(error) => {
                body_observation.finish(serde_json::json!({"stage":"bodyError","durationMs":body_observation.elapsed_ms(),"timeout":error.is_timeout()}));
                return Err(error.into());
            }
        };
        let parsed: ToolWireResponse = serde_json::from_str(&body)
            .map_err(|error| XaimError::Model(format!("工具响应 JSON 无效: {error}")))?;
        let mut choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| XaimError::Model("模型响应缺少 choices".into()))?;
        if choice.message.role != "assistant" {
            return Err(XaimError::Model("工具响应角色不是 assistant".into()));
        }
        if !self.deepseek {
            choice.message.reasoning_content = None;
        }
        Ok(ToolResponse {
            message: choice.message,
            finish_reason: choice.finish_reason.unwrap_or_default(),
            usage_reported: parsed.usage.is_some(),
            reasoning_tokens: parsed
                .usage
                .as_ref()
                .and_then(|u| u.completion_tokens_details.as_ref())
                .and_then(|details| details.reasoning_tokens),
            usage: parsed
                .usage
                .map(|u| Usage::new(u.prompt_tokens, u.completion_tokens))
                .unwrap_or_default(),
        })
        }.await;
        total.finish(serde_json::json!({"stage":"total","success":outcome.is_ok(),"durationMs":total.elapsed_ms()}));
        outcome
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        let payload = self.payload(&req, false);
        let resp = self
            .send_with_retries(&payload, Some(REQUEST_TIMEOUT), None, None)
            .await?;
        let body = resp.text().await?;
        let parsed: ChatCompletionResponse = serde_json::from_str(&body)
            .map_err(|e| XaimError::Model(format!("响应 JSON 解析失败: {e}; body: {body}")))?;
        let choice =
            parsed.choices.into_iter().next().ok_or_else(|| {
                XaimError::Model(format!("响应缺少 message.content; body: {body}"))
            })?;
        if choice.finish_reason.as_deref() == Some("length") {
            let reasoning_tokens = parsed
                .usage
                .as_ref()
                .and_then(|usage| usage.completion_tokens_details.as_ref())
                .and_then(|details| details.reasoning_tokens);
            let detail = reasoning_tokens
                .map(|tokens| format!("，其中思考消耗 {tokens} token"))
                .unwrap_or_default();
            return Err(XaimError::Model(format!(
                "模型 {} 的响应达到输出上限（{} token{detail}），正文未完整生成。请提高模型设置中的最大 token 数。",
                req.model, payload.max_tokens.or(payload.max_completion_tokens).unwrap_or(req.max_tokens)
            )));
        }
        let content = choice
            .message
            .content
            .ok_or_else(|| XaimError::Model("响应缺少 message.content".into()))?;
        if content.trim().is_empty() {
            return Err(XaimError::Model(format!(
                "模型 {} 返回空正文（结束原因：{}），未生成代码。",
                req.model,
                choice.finish_reason.as_deref().unwrap_or("未知")
            )));
        }
        let usage = parsed
            .usage
            .map(|u| Usage::new(u.prompt_tokens, u.completion_tokens))
            .unwrap_or_default();
        Ok(CompletionResponse { content, usage })
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let payload = self.payload(&req, true);
        let resp = self.send_with_retries(&payload, None, None, None).await?;

        let sse = Box::pin(resp.bytes_stream().eventsource());
        let chunks = futures::stream::try_unfold(sse, |mut sse| async move {
            loop {
                match sse.next().await {
                    Some(Ok(event)) => {
                        let data = event.data.trim();
                        if data == DONE_SENTINEL {
                            return Ok(None);
                        }
                        if data.is_empty() {
                            continue;
                        }
                        match parse_stream_delta(data) {
                            Ok(Some(text)) => return Ok(Some((text, sse))),
                            Ok(None) => continue,
                            Err(err) => return Err(err),
                        }
                    }
                    Some(Err(err)) => {
                        return Err(XaimError::Model(format!("SSE 流错误: {err}")));
                    }
                    None => return Ok(None),
                }
            }
        });
        Ok(Box::pin(chunks))
    }

    fn name(&self) -> &str {
        self.provider_name
    }

    // TODO: 按实际所选模型调整上下文窗口。
    fn max_context(&self) -> usize {
        128_000
    }
}

/// 发给 OpenAI 兼容接口的请求体。字段借用 [`CompletionRequest`] 以避免拷贝。
#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    messages: Vec<WireMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_thinking: Option<bool>,
    stream: bool,
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// 非流式响应。`usage` 允许缺失（个别兼容服务不返回）。
#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ResponseChoice>,
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct ToolWireResponse {
    choices: Vec<ToolWireChoice>,
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct ToolWireChoice {
    message: ToolMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseChoice {
    message: ResponseMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

/// 流式响应分片。部分服务的收尾块只有 usage、choices 为空数组。
#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: Delta,
}

#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ApiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    #[serde(default)]
    completion_tokens_details: Option<CompletionTokenDetails>,
}

#[derive(Deserialize)]
struct CompletionTokenDetails {
    reasoning_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    fn observed_request() -> ToolRequest {
        ToolRequest {
            model: "mock".into(),
            messages: vec![ToolMessage::text("user", "ping")],
            tools: vec![],
            max_tokens: 100,
        }
    }

    #[tokio::test]
    async fn observed_rate_limit_honors_retry_after_and_remaining_deadline() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(429)
            .with_header("retry-after", "2")
            .with_body("private gateway body")
            .expect(1)
            .create_async()
            .await;
        let events = std::sync::Mutex::new(Vec::<serde_json::Value>::new());
        let observer = |data| events.lock().unwrap().push(data);
        let error = provider_with(server.url())
            .complete_tools_observed(
                observed_request(),
                Some(Instant::now() + Duration::from_millis(250)),
                &observer,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, XaimError::Timeout(_)));
        mock.assert_async().await;
        let events = events.lock().unwrap();
        assert!(events
            .iter()
            .any(|e| e["stage"] == "headers" && e["status"] == 429));
        assert!(events.iter().any(|e| e["stage"] == "retrySkipped"));
        assert!(events
            .iter()
            .any(|e| e["stage"] == "total" && e["success"] == false));
        let text = serde_json::to_string(&*events).unwrap();
        assert!(
            !text.contains("test-key")
                && !text.contains("private gateway body")
                && !text.contains("ping")
        );
    }

    #[tokio::test]
    async fn observed_server_failures_record_each_attempt_and_body_timing() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(503)
            .with_body("unavailable")
            .expect(3)
            .create_async()
            .await;
        let events = std::sync::Mutex::new(Vec::<serde_json::Value>::new());
        let observer = |data| events.lock().unwrap().push(data);
        assert!(provider_with(server.url())
            .complete_tools_observed(observed_request(), None, &observer)
            .await
            .is_err());
        mock.assert_async().await;
        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e["stage"] == "attemptStarted")
                .count(),
            3
        );
        assert_eq!(
            events.iter().filter(|e| e["stage"] == "httpError").count(),
            3
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["stage"] == "retryScheduled")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn dropping_an_inflight_retry_records_cancellation() {
        let events = std::sync::Mutex::new(Vec::<serde_json::Value>::new());
        let observer = |data| events.lock().unwrap().push(data);
        assert!(tokio::time::timeout(
            Duration::from_millis(5),
            retry_wait(Duration::from_secs(1), None, Some(&observer), 1)
        )
        .await
        .is_err());
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["stage"] == "retry_wait" && e["cancelled"] == true));
    }
    #[tokio::test]
    async fn tool_wire_parameters_match_diagnostics_for_multiple_adapters() {
        for (adapter, expected) in [
            (
                "openai",
                serde_json::json!({"model":"o3-mini","max_completion_tokens":700}),
            ),
            (
                "deepseek",
                serde_json::json!({"model":"deepseek-flash","max_tokens":700,"thinking":{"type":"disabled"}}),
            ),
            (
                "qwen",
                serde_json::json!({"model":"qwen3-32b","max_tokens":700,"enable_thinking":false}),
            ),
        ] {
            let mut server = mockito::Server::new_async().await;
            let mock = server.mock("POST", "/chat/completions")
                .match_body(mockito::Matcher::PartialJson(expected.clone()))
                .with_status(200).with_header("content-type", "application/json")
                .with_body(serde_json::json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"done"}}],"usage":{"prompt_tokens":10,"completion_tokens":1}}).to_string())
                .create_async().await;
            let mut provider = provider_with(server.url());
            provider = match adapter {
                "deepseek" => provider.with_deepseek_defaults().with_thinking(Some(false)),
                "qwen" => provider.with_qwen_defaults().with_thinking(Some(false)),
                _ => provider.with_reasoning_model(true),
            };
            let req = ToolRequest {
                model: expected["model"].as_str().unwrap().into(),
                messages: vec![ToolMessage::text("user", "Create HTML page")],
                tools: vec![],
                max_tokens: 700,
            };
            let metadata = provider.tool_request_metadata(&req);
            assert_eq!(metadata["provider"], adapter);
            if adapter == "openai" {
                assert!(metadata["reasoningEffort"].is_null());
                assert_eq!(metadata["reasoningEffortSource"], "provider_default");
                assert_eq!(metadata["maxCompletionTokens"], 700);
                assert!(metadata["maxTokens"].is_null());
            } else {
                assert_eq!(metadata["maxTokens"], 700);
                assert!(metadata["reasoningEffort"].is_null());
                assert!(metadata["maxCompletionTokens"].is_null());
            }
            assert!(!metadata.to_string().contains("test-key"));
            assert!(!metadata.to_string().contains("Create HTML"));
            provider.complete_tools(req).await.unwrap();
            mock.assert_async().await;
        }
    }

    use super::*;

    #[tokio::test]
    async fn saved_sampling_and_caps_reach_completion_tool_and_stream_requests() {
        let mut server = Server::new_async().await;
        let provider = provider_with(server.url())
            .with_temperature(Some(0.25))
            .with_output_limit(Some(2048));
        let body = json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"pong"}}]});
        let complete = server
            .mock("POST", "/chat/completions")
            .match_body(Matcher::PartialJson(
                json!({"temperature":0.25,"max_tokens":8,"stream":false}),
            ))
            .with_status(200)
            .with_body(body.to_string())
            .create_async()
            .await;
        let mut request = CompletionRequest::new("custom", vec![Message::user("ping")]);
        request.max_tokens = 8;
        provider.complete(request.clone()).await.unwrap();
        complete.assert_async().await;
        let tools = server
            .mock("POST", "/chat/completions")
            .match_body(Matcher::PartialJson(
                json!({"temperature":0.25,"max_tokens":700,"tools":[],"stream":false}),
            ))
            .with_status(200)
            .with_body(body.to_string())
            .create_async()
            .await;
        provider
            .complete_tools(ToolRequest {
                model: "custom".into(),
                messages: vec![ToolMessage::text("user", "ping")],
                tools: vec![],
                max_tokens: 700,
            })
            .await
            .unwrap();
        tools.assert_async().await;
        let streaming = server
            .mock("POST", "/chat/completions")
            .match_body(Matcher::PartialJson(
                json!({"temperature":0.25,"max_tokens":8,"stream":true}),
            ))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: [DONE]\n\n")
            .create_async()
            .await;
        let mut stream = provider.stream(request).await.unwrap();
        assert!(stream.next().await.is_none());
        streaming.assert_async().await;
    }

    #[test]
    fn thinking_disabled_restores_sampling_and_never_injects_default_effort() {
        let provider = provider_with("http://localhost".into())
            .with_deepseek_defaults()
            .with_thinking(Some(false))
            .with_temperature(Some(0.5));
        let payload = provider
            .tool_payload(&ToolRequest {
                model: "deepseek-flash".into(),
                messages: vec![ToolMessage::text("user", "Create HTML")],
                tools: vec![],
                max_tokens: 700,
            })
            .unwrap();
        assert_eq!(payload["temperature"], json!(0.5));
        assert_eq!(payload["thinking"], json!({"type":"disabled"}));
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("max_completion_tokens").is_none());
    }

    #[tokio::test]
    async fn native_tool_calls_keep_deepseek_reasoning_only_in_wire_conversation() {
        let mut server = Server::new_async().await;
        let mock=server.mock("POST","/chat/completions")
            .match_body(Matcher::PartialJson(json!({"model":"deepseek-flash","max_tokens":8192,"tools":[{"type":"function","function":{"name":"list_dir","parameters":{"type":"object"}}}]})))
            .with_status(200).with_body(json!({"choices":[{"finish_reason":"tool_calls","message":{
                "role":"assistant","content":null,"reasoning_content":"private reasoning",
                "tool_calls":[{"id":"call-a","type":"function","function":{"name":"list_dir","arguments":"{\"path\":\".\"}"}}]}}],
                "usage":{"prompt_tokens":17,"completion_tokens":23,"completion_tokens_details":{"reasoning_tokens":11}}}).to_string()).create_async().await;
        let provider = provider_with(server.url()).with_deepseek_defaults();
        let response=provider.complete_tools(ToolRequest {model:"deepseek-flash".into(),messages:vec![ToolMessage::text("user","List files")],
            tools:vec![json!({"type":"function","function":{"name":"list_dir","parameters":{"type":"object"}}})],max_tokens:8192}).await.unwrap();
        assert_eq!(
            response.message.reasoning_content.as_deref(),
            Some("private reasoning")
        );
        assert_eq!(response.message.tool_calls[0].id, "call-a");
        assert_eq!(response.usage, Usage::new(17, 23));
        assert!(response.usage_reported);
        assert_eq!(response.reasoning_tokens, Some(11));
        mock.assert_async().await;
        let replay=server.mock("POST","/chat/completions")
            .match_body(Matcher::PartialJson(json!({"messages":[
                {"role":"assistant","reasoning_content":"private reasoning","tool_calls":[{"id":"call-a","type":"function","function":{"name":"list_dir","arguments":"{\"path\":\".\"}"}}]},
                {"role":"tool","tool_call_id":"call-a","content":"ok"}]})))
            .with_status(200).with_body(r#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"done"}}]}"#)
            .create_async().await;
        provider
            .complete_tools(ToolRequest {
                model: "deepseek-flash".into(),
                messages: vec![response.message, ToolMessage::result("call-a", "ok".into())],
                tools: vec![],
                max_tokens: 8192,
            })
            .await
            .unwrap();
        replay.assert_async().await;
    }
    use crate::model::provider::Message;
    use futures::StreamExt;
    use mockito::{Matcher, Server};
    use serde_json::json;

    fn provider_with(server_url: String) -> OpenAiProvider {
        OpenAiProvider::new(server_url, "test-key".to_string())
    }

    #[test]
    fn normalizes_gateway_endpoint_and_bearer_prefix() {
        assert_eq!(
            normalize_base_url(" https://example.test/newapi/v1/chat/completions/// "),
            "https://example.test/newapi/v1"
        );
        assert_eq!(normalize_api_key("  Bearer sk-test  "), "sk-test");
        assert_eq!(normalize_api_key("sk-test"), "sk-test");
    }

    fn sample_request() -> CompletionRequest {
        CompletionRequest::new(
            "deepseek-chat",
            vec![Message::system("你是助手"), Message::user("写个冒泡排序")],
        )
    }

    #[tokio::test]
    async fn reasoning_completion_sends_effort_without_unsupported_sampling_fields() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_body(Matcher::Json(json!({
                "model": "reasoner", "messages": [{"role": "user", "content": "hello"}],
                "reasoning_effort": "high", "max_completion_tokens": 4096, "stream": false
            })))
            .with_status(200)
            .with_body(r#"{"choices":[{"message":{"content":"done"}}]}"#)
            .create_async()
            .await;
        let provider = provider_with(server.url()).with_reasoning_effort(Some("high".into()));
        provider
            .complete(CompletionRequest::new(
                "reasoner",
                vec![Message::user("hello")],
            ))
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn malformed_or_missing_completion_content_is_not_retried() {
        for body in [
            "not json",
            r#"{"choices":[]}"#,
            r#"{"choices":[{"message":{"content":null}}]}"#,
        ] {
            let mut server = Server::new_async().await;
            let mock = server
                .mock("POST", "/chat/completions")
                .with_status(200)
                .with_body(body)
                .expect(1)
                .create_async()
                .await;
            let err = provider_with(server.url())
                .complete(sample_request())
                .await
                .unwrap_err();
            assert!(matches!(err, XaimError::Model(_)));
            mock.assert_async().await;
        }
    }

    #[tokio::test]
    async fn completion_without_usage_and_custom_request_fields() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", "Bearer test-key")
            .match_body(Matcher::Json(json!({
                "model": "custom", "messages": [{"role": "user", "content": "hello"}],
                "temperature": 0.25, "max_tokens": 17, "stream": false
            })))
            .with_status(200)
            .with_body(r#"{"choices":[{"message":{"content":"done"}}]}"#)
            .create_async()
            .await;
        let provider = provider_with(format!("{}/v1///", server.url()));
        let mut request = CompletionRequest::new("custom", vec![Message::user("hello")]);
        request.temperature = 0.25;
        request.max_tokens = 17;
        let response = provider.complete(request).await.unwrap();
        assert_eq!(response.content, "done");
        assert_eq!(response.usage, Usage::default());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn reasoning_budget_exhaustion_reports_truncation_instead_of_empty_diff() {
        for content in ["", "--- /dev/null\n+++ b/index.html\n"] {
            let mut server = Server::new_async().await;
            let mock = server.mock("POST", "/chat/completions")
                .with_status(200)
                .with_body(json!({"choices":[{"finish_reason":"length","message":{"content":content,"reasoning_content":"private thought"}}],
                    "usage":{"prompt_tokens":10,"completion_tokens":4096,"completion_tokens_details":{"reasoning_tokens":4096}}}).to_string())
                .expect(1).create_async().await;
            let error = provider_with(server.url())
                .complete(sample_request())
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("达到输出上限"));
            assert!(error.contains("思考消耗 4096"));
            assert!(!error.contains("private thought"));
            mock.assert_async().await;
        }
    }

    #[tokio::test]
    async fn empty_final_content_is_rejected_without_exposing_reasoning() {
        let mut server = Server::new_async().await;
        let mock = server.mock("POST", "/chat/completions").with_status(200)
            .with_body(r#"{"choices":[{"finish_reason":"stop","message":{"content":"","reasoning_content":"private thought"}}]}"#)
            .expect(1).create_async().await;
        let error = provider_with(server.url())
            .complete(sample_request())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("空正文"));
        assert!(!error.contains("private thought"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn deepseek_thinking_default_budget_and_saved_limit_are_sent() {
        for (model, limit, expected) in [
            ("deepseek-flash", None, 32768),
            ("deepseek-v4-pro", None, 32768),
            ("deepseek-chat", None, 4096),
            ("deepseek-flash", Some(8192), 4096),
        ] {
            let mut server = Server::new_async().await;
            let mock = server
                .mock("POST", "/chat/completions")
                .match_body(Matcher::PartialJson(
                    json!({"model":model,"max_tokens":expected}),
                ))
                .with_status(200)
                .with_body(r#"{"choices":[{"finish_reason":"stop","message":{"content":"done"}}]}"#)
                .expect(1)
                .create_async()
                .await;
            let provider = provider_with(server.url())
                .with_deepseek_defaults()
                .with_output_limit(limit);
            let request = CompletionRequest::new(model, vec![Message::user("hello")]);
            if model != "deepseek-chat" {
                let payload = serde_json::to_value(provider.payload(&request, false)).unwrap();
                assert!(payload.get("temperature").is_none());
                assert!(payload.get("max_completion_tokens").is_none());
            }
            provider.complete(request).await.unwrap();
            mock.assert_async().await;
        }
    }

    #[tokio::test]
    async fn rate_limit_and_request_timeout_exhaust_exactly_two_retries() {
        for status in [408, 429] {
            let mut server = Server::new_async().await;
            let mock = server
                .mock("POST", "/chat/completions")
                .with_status(status)
                .with_body("retry later")
                .expect(3)
                .create_async()
                .await;
            let error = provider_with(server.url())
                .complete(sample_request())
                .await
                .unwrap_err();
            assert!(
                matches!(error, XaimError::Model(ref m) if m.contains(&status.to_string()) && m.contains("retry later"))
            );
            mock.assert_async().await;
        }
    }

    #[tokio::test]
    async fn stream_stops_at_done_even_if_more_events_follow() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_header("content-type", "text/event-stream")
            .with_body(concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
                "data: [DONE]\n\n",
                "data: invalid json must never be parsed\n\n"
            ))
            .create_async()
            .await;
        let mut stream = provider_with(server.url())
            .stream(sample_request())
            .await
            .unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap(), "hello");
        assert!(stream.next().await.is_none());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn complete_returns_content_and_usage() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_header("authorization", "Bearer test-key")
            .match_body(Matcher::PartialJson(json!({
                "model": "deepseek-chat",
                "temperature": 0.0,
                "max_tokens": 4096,
                "stream": false,
                "messages": [
                    {"role": "system", "content": "你是助手"},
                    {"role": "user", "content": "写个冒泡排序"}
                ],
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "choices": [{"message": {"role": "assistant", "content": "排序结果"}}],
                    "usage": {"prompt_tokens": 12, "completion_tokens": 34}
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = provider_with(server.url());
        let resp = provider.complete(sample_request()).await.unwrap();
        assert_eq!(resp.content, "排序结果");
        assert_eq!(resp.usage, Usage::new(12, 34));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn complete_error_includes_status_and_body() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(400)
            .with_body(r#"{"error":{"message":"model not found"}}"#)
            .create_async()
            .await;

        let provider = provider_with(server.url());
        let msg = provider
            .complete(sample_request())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("400") && msg.contains("model not found"),
            "实际错误: {msg}"
        );
        mock.assert_async().await; // 400 不可重试，只发一次
    }

    #[tokio::test]
    async fn retries_5xx_twice_with_backoff_before_giving_up() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(500)
            .with_body("server exploded")
            .expect(3) // 首次 + 2 次重试
            .create_async()
            .await;

        let provider = provider_with(server.url());
        let msg = provider
            .complete(sample_request())
            .await
            .unwrap_err()
            .to_string();
        assert!(msg.contains("server exploded"), "实际错误: {msg}");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn transport_errors_retry_then_report_http_error() {
        // 端口 9（discard）无人监听，连接被拒：重试耗尽后归为 XaimError::Http。
        let provider = OpenAiProvider::new("http://127.0.0.1:9".to_string(), "k".to_string());
        let err = provider.complete(sample_request()).await.unwrap_err();
        assert!(matches!(err, XaimError::Http(_)));
    }

    #[tokio::test]
    async fn stream_reassembles_sse_chunks() {
        let mut server = Server::new_async().await;
        let sse_body = concat!(
            r#"data: {"choices":[{"delta":{"role":"assistant","content":""}}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"content":"He"}}]}"#,
            "\n\n",
            r#"data: {"choices":[{"delta":{"content":"llo"}}]}"#,
            "\n\n",
            r#"data: {"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let mock = server
            .mock("POST", "/chat/completions")
            .match_body(Matcher::PartialJson(json!({"stream": true})))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_body)
            .create_async()
            .await;

        let provider = provider_with(server.url());
        let mut stream = provider.stream(sample_request()).await.unwrap();
        let mut collected = String::new();
        while let Some(item) = stream.next().await {
            collected.push_str(&item.unwrap());
        }
        assert_eq!(collected, "Hello");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn stream_surfaces_error_chunk_with_raw_body() {
        let mut server = Server::new_async().await;
        let sse_body = concat!(
            r#"data: {"choices":[{"delta":{"content":"部分"}}]}"#,
            "\n\n",
            r#"data: {"error":{"message":"余额不足","type":"insufficient_quota"}}"#,
            "\n\n",
        );
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_body)
            .create_async()
            .await;

        let provider = provider_with(server.url());
        let mut stream = provider.stream(sample_request()).await.unwrap();
        let err = stream.next().await.unwrap().unwrap(); // 第一个增量正常
        assert_eq!(err, "部分");
        let err = stream.next().await.unwrap().unwrap_err();
        assert!(
            matches!(err, XaimError::Model(ref m) if m.contains("余额不足")),
            "实际错误: {err}"
        );
    }

    #[tokio::test]
    async fn stream_request_error_includes_body() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(401)
            .with_body(r#"{"error":{"message":"invalid api key"}}"#)
            .create_async()
            .await;

        let provider = provider_with(server.url());
        let err = provider
            .stream(sample_request())
            .await
            .err()
            .expect("stream 应返回配置的错误");
        assert!(matches!(err, XaimError::Model(ref m) if m.contains("invalid api key")));
        mock.assert_async().await;
    }
}
