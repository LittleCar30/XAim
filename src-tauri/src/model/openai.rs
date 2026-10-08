//! OpenAI 兼容提供方：实现 Chat Completions 协议（`POST {base_url}/chat/completions`）。
//!
//! 同一套实现覆盖所有兼容该协议的服务，仅 base_url 不同：
//! OpenAI 官方（`https://api.openai.com/v1`）、
//! DeepSeek（`https://api.deepseek.com/v1`）、
//! Ollama 本地模型（`http://localhost:11434/v1`）等。

use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use super::provider::{CompletionRequest, CompletionResponse, ModelProvider, Usage};
use crate::errors::{Result, XaimError};

/// 单次请求超时：非流式补全的整体上限；流式请求用它限制读空闲
/// （120s 内未收到任何字节即断开），整体时长不设上限以免截断长生成。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// 失败后的重试次数（不含首次请求）。
const MAX_RETRIES: u32 = 2;
/// 指数退避基数：第 n 次重试前等待 `BACKOFF_BASE_MS * 2^(n-1)` 毫秒。
const BACKOFF_BASE_MS: u64 = 500;
/// OpenAI 协议的流结束哨兵。
const DONE_SENTINEL: &str = "[DONE]";

/// OpenAI 兼容提供方。
pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
    reasoning_effort: Option<String>,
    reasoning_model: bool,
}

impl OpenAiProvider {
    pub fn new(base_url: String, api_key: String) -> Self {
        // 不设客户端级总超时：流式响应的时长由内容决定；空闲保护交给 read_timeout。
        let client = reqwest::Client::builder()
            .read_timeout(REQUEST_TIMEOUT)
            .build()
            .expect("构建 reqwest Client 失败");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            client,
            reasoning_effort: None,
            reasoning_model: false,
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

    /// 构造时使用的 API 地址。
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 发送请求并在可重试的失败（网络错误、408/429/5xx）上指数退避重试。
    /// 只重试到拿到成功响应为止，响应体的消费不在重试范围内。
    /// `timeout` 为整体超时：非流式传 [`REQUEST_TIMEOUT`]，流式传 `None`
    /// （流式只受客户端 read_timeout 的空闲保护）。
    async fn send_with_retries(
        &self,
        payload: &WireRequest<'_>,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut attempt = 0u32;
        loop {
            let mut request = self
                .client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(payload);
            if let Some(timeout) = timeout {
                request = request.timeout(timeout);
            }
            let send = request.send().await;

            match send {
                Ok(resp) if resp.status().is_success() => return Ok(resp),
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "(读取响应体失败)".to_string());
                    if attempt < MAX_RETRIES && is_retryable_status(status) {
                        attempt += 1;
                        tracing::warn!(attempt, %status, "请求失败，稍后重试");
                        tokio::time::sleep(backoff_delay(attempt)).await;
                        continue;
                    }
                    return Err(XaimError::Model(format!("HTTP {status}: {body}")));
                }
                Err(err) => {
                    let retryable = err.is_connect() || err.is_body() || err.is_request();
                    if attempt < MAX_RETRIES && retryable {
                        attempt += 1;
                        tracing::warn!(attempt, error = %err, "网络错误，稍后重试");
                        tokio::time::sleep(backoff_delay(attempt)).await;
                        continue;
                    }
                    return Err(XaimError::Http(err));
                }
            }
        }
    }
}

/// 可重试的状态码：请求超时、限流与服务端临时故障。
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429) || status.is_server_error()
}

fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(BACKOFF_BASE_MS << (attempt - 1))
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
        stream,
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        let mut payload = wire_request(&req, false);
        if self.reasoning_model || self.reasoning_effort.is_some() {
            payload.reasoning_effort = self.reasoning_effort.as_deref();
            payload.temperature = None;
            payload.max_tokens = None;
            payload.max_completion_tokens = Some(req.max_tokens);
        }
        let resp = self
            .send_with_retries(&payload, Some(REQUEST_TIMEOUT))
            .await?;
        let body = resp.text().await?;
        let parsed: ChatCompletionResponse = serde_json::from_str(&body)
            .map_err(|e| XaimError::Model(format!("响应 JSON 解析失败: {e}; body: {body}")))?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| XaimError::Model(format!("响应缺少 message.content; body: {body}")))?;
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
        let mut payload = wire_request(&req, true);
        if self.reasoning_model || self.reasoning_effort.is_some() {
            payload.reasoning_effort = self.reasoning_effort.as_deref();
            payload.temperature = None;
            payload.max_tokens = None;
            payload.max_completion_tokens = Some(req.max_tokens);
        }
        let resp = self.send_with_retries(&payload, None).await?;

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
        "openai"
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
struct ResponseChoice {
    message: ResponseMessage,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::provider::Message;
    use futures::StreamExt;
    use mockito::{Matcher, Server};
    use serde_json::json;

    fn provider_with(server_url: String) -> OpenAiProvider {
        OpenAiProvider::new(server_url, "test-key".to_string())
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
            assert!(matches!(err, XaimError::Model(ref message) if message.contains(body)));
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
            .with_body(r#"{"choices":[{"message":{"content":""}}]}"#)
            .create_async()
            .await;
        let provider = provider_with(format!("{}/v1///", server.url()));
        let mut request = CompletionRequest::new("custom", vec![Message::user("hello")]);
        request.temperature = 0.25;
        request.max_tokens = 17;
        let response = provider.complete(request).await.unwrap();
        assert_eq!(response.content, "");
        assert_eq!(response.usage, Usage::default());
        mock.assert_async().await;
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
