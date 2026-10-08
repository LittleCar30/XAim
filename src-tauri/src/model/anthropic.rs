//! Anthropic Claude API。TODO: 接入实际 HTTP 调用、鉴权与流式响应。

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use super::provider::{CompletionRequest, CompletionResponse, ModelProvider};
use crate::errors::Result;

/// Anthropic 提供方。
pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    /// 构造时使用的 API 地址。
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 构造时使用的 API key。
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
        Err(crate::errors::XaimError::Model(
            "Anthropic 原生接入尚未实现，请使用已配置的兼容端点".into(),
        ))
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        Err(crate::errors::XaimError::Model(
            "Anthropic 流式接入尚未实现".into(),
        ))
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    // TODO: 按实际所选模型调整上下文窗口。
    fn max_context(&self) -> usize {
        200_000
    }
}
