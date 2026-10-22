//! 原生 Anthropic Messages API 适配器。

use std::pin::Pin;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::{stream, Stream};
use serde_json::{json, Value};

use super::provider::{
    CompletionRequest, CompletionResponse, FunctionCall, Message, ModelProvider, RequestObserver,
    ToolCall, ToolMessage, ToolRequest, ToolResponse, Usage,
};
use crate::errors::{Result, XaimError};

const API_VERSION: &str = "2023-06-01";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_RETRIES: u32 = 2;

/// Anthropic 原生 Messages API。
pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.trim().to_string(),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(REQUEST_TIMEOUT)
                .build()
                .expect("构建 Anthropic reqwest Client 失败"),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    async fn send(&self, payload: Value, deadline: Option<Instant>) -> Result<reqwest::Response> {
        for attempt in 0..=MAX_RETRIES {
            let mut request = self
                .client
                .post(format!("{}/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .json(&payload);
            if let Some(deadline) = deadline {
                request = request.timeout(deadline.saturating_duration_since(Instant::now()));
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(error)
                    if attempt < MAX_RETRIES && (error.is_connect() || error.is_timeout()) =>
                {
                    tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
                    continue;
                }
                Err(error) => return Err(XaimError::from(error)),
            };
            if response.status().is_success() {
                return Ok(response);
            }
            let status = response.status();
            let retryable =
                matches!(status.as_u16(), 408 | 409 | 429 | 529) || status.is_server_error();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "(读取响应体失败)".into());
            if retryable && attempt < MAX_RETRIES {
                tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
                continue;
            }
            return Err(XaimError::Model(format!("Anthropic HTTP {status}: {body}")));
        }
        unreachable!("Anthropic retry loop always returns")
    }

    async fn complete_value(&self, payload: Value, deadline: Option<Instant>) -> Result<Value> {
        let response = self.send(payload, deadline).await?;
        let body = response.text().await.map_err(XaimError::from)?;
        serde_json::from_str(&body)
            .map_err(|error| XaimError::Model(format!("Anthropic 响应 JSON 解析失败: {error}")))
    }
}

fn text_block(text: &str) -> Value {
    json!({"type":"text","text":text})
}

fn content_blocks(message: &ToolMessage) -> Vec<Value> {
    if message.role == "tool" {
        return vec![json!({
            "type":"tool_result","tool_use_id":message.tool_call_id,"content":message.content.as_deref().unwrap_or_default()
        })];
    }
    let mut blocks = Vec::new();
    if let Some(content) = message.content.as_deref().filter(|text| !text.is_empty()) {
        blocks.push(text_block(content));
    }
    for call in &message.tool_calls {
        let input = serde_json::from_str::<Value>(&call.function.arguments)
            .unwrap_or_else(|_| json!({"_invalid_arguments":call.function.arguments}));
        blocks
            .push(json!({"type":"tool_use","id":call.id,"name":call.function.name,"input":input}));
    }
    if blocks.is_empty() {
        blocks.push(text_block(""));
    }
    blocks
}

fn tool_messages(messages: &[ToolMessage]) -> (Option<String>, Vec<Value>) {
    let mut system = Vec::new();
    let mut output = Vec::new();
    for message in messages {
        if message.role == "system" {
            if let Some(content) = message.content.as_deref() {
                system.push(content.to_string());
            }
            continue;
        }
        let role = if message.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        output.push(json!({"role":role,"content":content_blocks(message)}));
    }
    (
        if system.is_empty() {
            None
        } else {
            Some(system.join("\n\n"))
        },
        output,
    )
}

fn completion_messages(messages: &[Message]) -> (Option<String>, Vec<Value>) {
    let mut system = Vec::new();
    let mut output = Vec::new();
    for message in messages {
        if message.role == "system" {
            system.push(message.content.clone());
        } else {
            let role = if message.role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            output.push(json!({"role":role,"content":message.content}));
        }
    }
    (
        if system.is_empty() {
            None
        } else {
            Some(system.join("\n\n"))
        },
        output,
    )
}

fn anthropic_tools(tools: &[Value]) -> Vec<Value> {
    tools.iter().filter_map(|tool| {
        let function = tool.get("function")?;
        Some(json!({
            "name":function.get("name")?,
            "description":function.get("description").cloned().unwrap_or(Value::Null),
            "input_schema":function.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object"}))
        }))
    }).collect()
}

fn parse_tool_response(body: Value) -> Result<ToolResponse> {
    let content = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| XaimError::Model("Anthropic 响应缺少 content".into()))?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            Some("tool_use") => {
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                calls.push(ToolCall {
                    id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        arguments: serde_json::to_string(&input)?,
                    },
                });
            }
            Some(other) => {
                return Err(XaimError::Model(format!(
                    "Anthropic 返回未知内容块：{other}"
                )))
            }
            None => return Err(XaimError::Model("Anthropic 内容块缺少 type".into())),
        }
    }
    let finish_reason = match body
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        "end_turn" | "stop_sequence" => "stop",
        other => other,
    };
    let usage = body.get("usage").cloned().unwrap_or_else(|| json!({}));
    Ok(ToolResponse {
        message: ToolMessage {
            responses_output: vec![],
            role: "assistant".into(),
            content: (!text.is_empty()).then_some(text),
            tool_calls: calls,
            tool_call_id: None,
            reasoning_content: None,
        },
        finish_reason: finish_reason.into(),
        usage: Usage::new(
            usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default() as u32,
            usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default() as u32,
        ),
        usage_reported: body.get("usage").is_some(),
        reasoning_tokens: None,
    })
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn supports_checkpoint_context(&self) -> bool {
        true
    }
    fn supports_tools(&self) -> bool {
        true
    }

    fn tool_request_metadata(&self, req: &ToolRequest) -> Value {
        json!({"provider":"anthropic","protocol":"messages","model":req.model,"endpoint":format!("{}/messages",self.base_url),"auth":"x-api-key","maxTokens":req.max_tokens,"stream":false})
    }

    fn interaction_request_metadata(&self, req: &ToolRequest) -> Value {
        self.tool_request_metadata(req)
    }

    async fn complete_tools_observed(
        &self,
        req: ToolRequest,
        deadline: Option<Instant>,
        observer: &RequestObserver<'_>,
    ) -> Result<ToolResponse> {
        let (system, messages) = tool_messages(&req.messages);
        let mut payload = json!({"model":req.model,"max_tokens":req.max_tokens,"messages":messages,"tools":anthropic_tools(&req.tools)});
        if let Some(system) = system {
            payload["system"] = json!(system);
        }
        observer(
            json!({"stage":"requestParameters","parameters":self.tool_request_metadata(&req)}),
        );
        parse_tool_response(self.complete_value(payload, deadline).await?)
    }

    async fn complete_tools(&self, req: ToolRequest) -> Result<ToolResponse> {
        self.complete_tools_observed(req, None, &|_| {}).await
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        let (system, messages) = completion_messages(&req.messages);
        let mut payload =
            json!({"model":req.model,"max_tokens":req.max_tokens,"messages":messages});
        if let Some(system) = system {
            payload["system"] = json!(system);
        }
        if req.temperature > 0.0 {
            payload["temperature"] = json!(req.temperature);
        }
        let response = parse_tool_response(self.complete_value(payload, None).await?)?;
        Ok(CompletionResponse {
            content: response.message.content.unwrap_or_default(),
            usage: response.usage,
        })
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let response = self.complete(req).await?;
        Ok(Box::pin(stream::once(async move { Ok(response.content) })))
    }

    fn name(&self) -> &str {
        "anthropic"
    }
    fn max_context(&self) -> usize {
        200_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    fn request() -> ToolRequest {
        ToolRequest {
            model: "claude-sonnet-4-5".into(),
            messages: vec![
                ToolMessage::text("system", "follow the tool contract"),
                ToolMessage::text("user", "read the file"),
            ],
            tools: vec![
                json!({"type":"function","function":{"name":"read_file","description":"read","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}}),
            ],
            max_tokens: 1024,
        }
    }

    #[tokio::test]
    async fn native_messages_request_maps_roles_headers_and_tool_schema() {
        let mut server = Server::new_async().await;
        let mock = server.mock("POST", "/v1/messages")
            .match_header("x-api-key","sk-test").match_header("anthropic-version",API_VERSION)
            .match_body(Matcher::PartialJson(json!({"model":"claude-sonnet-4-5","max_tokens":1024,"system":"follow the tool contract","tools":[{"name":"read_file","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}]})))
            .with_status(200).with_body(json!({"stop_reason":"tool_use","content":[{"type":"text","text":"I will read it"},{"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"a.txt"}}],"usage":{"input_tokens":12,"output_tokens":7}}).to_string()).create_async().await;
        let provider = AnthropicProvider::new(server.url() + "/v1", "sk-test".into());
        let response = provider.complete_tools(request()).await.unwrap();
        mock.assert_async().await;
        assert_eq!(response.finish_reason, "tool_calls");
        assert_eq!(response.message.tool_calls[0].function.name, "read_file");
        assert_eq!(response.usage, Usage::new(12, 7));
    }

    #[tokio::test]
    async fn tool_result_round_trip_uses_anthropic_tool_result_block() {
        let mut server = Server::new_async().await;
        let mock = server.mock("POST", "/v1/messages")
            .match_body(Matcher::PartialJson(json!({"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"a.txt"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"file contents"}]}]})))
            .with_status(200).with_body(json!({"stop_reason":"end_turn","content":[{"type":"text","text":"done"}]}).to_string()).create_async().await;
        let mut req = request();
        req.messages = vec![
            ToolMessage {
                responses_output: vec![],
                role: "assistant".into(),
                content: None,
                tool_calls: vec![ToolCall {
                    id: "toolu_1".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "read_file".into(),
                        arguments: r#"{"path":"a.txt"}"#.into(),
                    },
                }],
                tool_call_id: None,
                reasoning_content: None,
            },
            ToolMessage::result("toolu_1", "file contents".into()),
        ];
        let provider = AnthropicProvider::new(server.url() + "/v1", "sk-test".into());
        let response = provider.complete_tools(req).await.unwrap();
        mock.assert_async().await;
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.message.content.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn anthropic_errors_are_visible_without_exposing_key() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(401)
            .with_body("invalid key")
            .create_async()
            .await;
        let provider = AnthropicProvider::new(server.url() + "/v1", "sk-secret".into());
        let error = provider
            .complete_tools(request())
            .await
            .unwrap_err()
            .to_string();
        mock.assert_async().await;
        assert!(error.contains("401"));
        assert!(!error.contains("sk-secret"));
    }

    #[tokio::test]
    async fn transient_anthropic_errors_use_bounded_retries() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(529)
            .with_body("overloaded")
            .expect(3)
            .create_async()
            .await;
        let provider = AnthropicProvider::new(server.url() + "/v1", "sk-test".into());
        let error = provider
            .complete_tools(request())
            .await
            .unwrap_err()
            .to_string();
        mock.assert_async().await;
        assert!(error.contains("529"));
    }
}
