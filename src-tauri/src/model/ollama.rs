//! Ollama 本地模型。TODO: 接入本地 HTTP 接口与模型列表发现。

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use super::provider::{CompletionRequest, CompletionResponse, ModelProvider};
use crate::errors::Result;

/// Ollama 提供方。
pub struct OllamaProvider;

#[async_trait]
impl ModelProvider for OllamaProvider {
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
        todo!("model::ollama::complete")
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        todo!("model::ollama::stream")
    }

    fn name(&self) -> &str {
        "ollama"
    }

    // TODO: 按实际加载的本地模型调整上下文窗口。
    fn max_context(&self) -> usize {
        8_192
    }
}
