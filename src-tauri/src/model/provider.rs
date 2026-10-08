//! 模型提供方统一接口：所有 provider（OpenAI/Anthropic/DeepSeek/Ollama…）
//! 实现同一 trait，上层 agent 与 router 只依赖此抽象。

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::errors::Result;

#[cfg(test)]
use crate::errors::XaimError;

/// 所有模型提供方实现的统一 trait。
///
/// `complete` 一次性返回完整补全；`stream` 返回增量文本分片流，
/// 流中任一分片出错即以 `Err(XaimError)` 结束。各方法均为
/// `&self`，provider 实例可安全地跨任务共享。
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn supports_tools(&self) -> bool {
        false
    }

    async fn complete_tools(&self, _req: ToolRequest) -> Result<ToolResponse> {
        Err(crate::errors::XaimError::Model(
            "当前端点未实现工具调用".into(),
        ))
    }

    /// 发送补全请求，阻塞至模型输出完毕并返回全文与 token 用量。
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse>;

    /// 发送补全请求，返回逐段增量的文本流。
    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>>;

    /// 提供方唯一标识（如 `"openai"`、`"anthropic"`）。
    fn name(&self) -> &str;

    /// 该提供方支持的上下文窗口大小（token 数上限）。
    fn max_context(&self) -> usize;
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl ToolMessage {
    pub fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: Some(content.into()),
            tool_calls: vec![],
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn result(id: &str, content: String) -> Self {
        let mut message = Self::text("tool", content);
        message.tool_call_id = Some(id.into());
        message
    }
}

#[derive(Debug, Clone)]
pub struct ToolRequest {
    pub model: String,
    pub messages: Vec<ToolMessage>,
    pub tools: Vec<serde_json::Value>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct ToolResponse {
    pub message: ToolMessage,
    pub finish_reason: String,
    pub usage: Usage,
}

/// 一次补全请求。
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    /// 本次调用使用的模型名（如 `"claude-sonnet-4"`）。
    pub model: String,
    /// 按时间顺序排列的对话消息。
    pub messages: Vec<Message>,
    /// 采样温度。
    pub temperature: f32,
    /// 最大生成 token 数。
    pub max_tokens: u32,
}

impl CompletionRequest {
    /// 以确定性的默认采样参数构造请求（temperature=0，max_tokens=4096），
    /// 需要其他取值时直接对字段赋值。
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            temperature: 0.0,
            max_tokens: 4096,
        }
    }
}

/// 对话中的一条消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// 角色名：`"system"` / `"user"` / `"assistant"`。
    pub role: String,
    /// 消息正文。
    pub content: String,
}

impl Message {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new("system", content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new("user", content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new("assistant", content)
    }
}

/// 一次补全的完整结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResponse {
    /// 模型输出的完整文本。
    pub content: String,
    /// 本次调用的 token 用量。
    pub usage: Usage,
}

/// token 用量统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// 输入（提示词）消耗的 token 数。
    pub input_tokens: u32,
    /// 输出（补全）消耗的 token 数。
    pub output_tokens: u32,
}

impl Usage {
    pub fn new(input_tokens: u32, output_tokens: u32) -> Self {
        Self {
            input_tokens,
            output_tokens,
        }
    }
}

/// 测试用的 Mock 提供方：仅在 `cargo test` 下编译，
/// 返回预先设定好的文本，用于上层逻辑（agent/router/命令）的单元测试。
#[cfg(test)]
pub struct MockProvider {
    name: String,
    max_context: usize,
    response: String,
    /// 流式输出时每个分片的最大字符数。
    chunk_chars: usize,
    error: Option<String>,
}

#[cfg(test)]
impl MockProvider {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            name: "mock".to_string(),
            max_context: 8192,
            response: response.into(),
            chunk_chars: 8,
            error: None,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_max_context(mut self, max_context: usize) -> Self {
        self.max_context = max_context;
        self
    }

    pub fn with_chunk_chars(mut self, chunk_chars: usize) -> Self {
        self.chunk_chars = chunk_chars.max(1);
        self
    }

    /// 设定后 `complete`/`stream` 都返回 [`XaimError::Model`]，用于测试失败路径。
    pub fn with_error(mut self, message: impl Into<String>) -> Self {
        self.error = Some(message.into());
        self
    }
}

#[cfg(test)]
#[async_trait]
impl ModelProvider for MockProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        if let Some(message) = &self.error {
            return Err(XaimError::Model(message.clone()));
        }
        let input_tokens: u32 = req
            .messages
            .iter()
            .map(|m| m.content.chars().count() as u32)
            .sum();
        Ok(CompletionResponse {
            content: self.response.clone(),
            usage: Usage::new(input_tokens, self.response.chars().count() as u32),
        })
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        if let Some(message) = &self.error {
            return Err(XaimError::Model(message.clone()));
        }
        let chunks: Vec<String> = self
            .response
            .chars()
            .collect::<Vec<_>>()
            .chunks(self.chunk_chars)
            .map(|chunk| chunk.iter().collect())
            .collect();
        Ok(Box::pin(futures::stream::iter(
            chunks.into_iter().map(Ok::<String, XaimError>),
        )))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn max_context(&self) -> usize {
        self.max_context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn empty_mock_response_has_no_stream_chunks_or_output_usage() {
        let mock = MockProvider::new("").with_chunk_chars(0);
        let request = CompletionRequest::new("mock", vec![]);
        assert_eq!(
            mock.complete(request.clone()).await.unwrap().usage,
            Usage::default()
        );
        let mut stream = mock.stream(request).await.unwrap();
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn mock_completes_with_usage() {
        let mock = MockProvider::new("hello world");
        let req = CompletionRequest::new(
            "mock-model",
            vec![Message::system("sys"), Message::user("hi")],
        );
        let resp = mock.complete(req).await.unwrap();
        assert_eq!(resp.content, "hello world");
        assert_eq!(resp.usage.input_tokens, 5); // "sys" + "hi" 的字符数
        assert_eq!(resp.usage.output_tokens, 11);
    }

    #[tokio::test]
    async fn mock_streams_chunks_in_order_and_reassembles() {
        let mock = MockProvider::new("你好，世界").with_chunk_chars(2);
        let mut stream = mock
            .stream(CompletionRequest::new("mock-model", vec![]))
            .await
            .unwrap();
        let mut collected = String::new();
        while let Some(item) = stream.next().await {
            collected.push_str(&item.unwrap());
        }
        assert_eq!(collected, "你好，世界");
    }

    #[tokio::test]
    async fn mock_reports_configured_error() {
        let mock = MockProvider::new("ignored").with_error("429 限流");
        let req = CompletionRequest::new("mock-model", vec![]);

        let complete_err = mock.complete(req.clone()).await.unwrap_err();
        assert!(matches!(complete_err, XaimError::Model(ref m) if m == "429 限流"));

        let stream_err = mock
            .stream(req)
            .await
            .err()
            .expect("stream 应返回配置的错误");
        assert!(matches!(stream_err, XaimError::Model(ref m) if m == "429 限流"));
    }

    #[tokio::test]
    async fn mock_is_usable_as_trait_object() {
        let provider: Box<dyn ModelProvider> = Box::new(
            MockProvider::new("ok")
                .with_name("mock-1")
                .with_max_context(1024),
        );
        assert_eq!(provider.name(), "mock-1");
        assert_eq!(provider.max_context(), 1024);

        let mut stream = provider
            .stream(CompletionRequest::new("m", vec![Message::user("go")]))
            .await
            .unwrap();
        let mut collected = String::new();
        while let Some(item) = stream.next().await {
            collected.push_str(&item.unwrap());
        }
        assert_eq!(collected, "ok");
    }

    #[test]
    fn message_constructors_set_roles() {
        assert_eq!(Message::system("s"), Message::new("system", "s"));
        assert_eq!(Message::user("u"), Message::new("user", "u"));
        assert_eq!(Message::assistant("a"), Message::new("assistant", "a"));
    }
}
