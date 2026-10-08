//! DeepSeek API。TODO: 接入实际 HTTP 调用与流式响应。

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use super::provider::{CompletionRequest, CompletionResponse, ModelProvider};
use crate::errors::Result;

/// DeepSeek 提供方。
pub struct DeepSeekProvider;

#[async_trait]
impl ModelProvider for DeepSeekProvider {
    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
        todo!("model::deepseek::complete")
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        todo!("model::deepseek::stream")
    }

    fn name(&self) -> &str {
        "deepseek"
    }

    // TODO: 按实际所选模型调整上下文窗口。
    fn max_context(&self) -> usize {
        64_000
    }
}
