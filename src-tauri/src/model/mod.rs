//! 模型层：提供方抽象、各家实现、路由与工厂。

pub mod anthropic;
pub mod capabilities;
pub mod deepseek;
pub mod ollama;
pub mod openai;
pub mod provider;
pub mod router;

use self::anthropic::AnthropicProvider;
use self::openai::OpenAiProvider;
use self::provider::ModelProvider;
use crate::config::model::ModelConfig;
use crate::errors::{Result, XaimError};

/// 工厂支持的 provider 标识，同时用于错误提示。
pub const SUPPORTED_PROVIDERS: [&str; 6] = [
    "openai",
    "deepseek",
    "qwen",
    "ollama",
    "anthropic",
    "newapi",
];

/// 各 provider 的默认 API 地址。
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
const QWEN_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/v1";

/// Ollama 本地服务不校验鉴权，但 OpenAI 兼容协议要求非空 Bearer，用占位符补位。
const OLLAMA_PLACEHOLDER_KEY: &str = "ollama";

/// 根据模型配置创建 provider 实例。
///
/// `provider` 取 [`SUPPORTED_PROVIDERS`] 中的标识；DeepSeek / Qwen / Ollama
/// 均为 OpenAI 兼容协议，复用 [`OpenAiProvider`]，仅默认地址不同；
/// 配置里的 `base_url` 可覆盖默认地址（代理、私有化部署）。
/// 云端 provider 要求非空 api key，Ollama 可留空。
pub fn create_provider(config: &ModelConfig) -> Result<Box<dyn ModelProvider>> {
    let base_url = resolve_base_url(config)?;
    match config.provider.as_str() {
        "openai" => Ok(Box::new(
            OpenAiProvider::new(base_url, require_api_key(config)?)
                .with_reasoning_effort(config.reasoning_effort.clone())
                .with_reasoning_model(config.reasoning_model)
                .with_output_limit(config.max_tokens)
                .with_temperature(config.temperature),
        )),
        "deepseek" => Ok(Box::new(
            OpenAiProvider::new(base_url, require_api_key(config)?)
                .with_deepseek_defaults()
                .with_reasoning_effort(config.reasoning_effort.clone())
                .with_thinking(config.thinking)
                .with_output_limit(config.max_tokens)
                .with_temperature(config.temperature),
        )),
        "qwen" => Ok(Box::new(
            OpenAiProvider::new(base_url, require_api_key(config)?)
                .with_qwen_defaults()
                .with_thinking(config.thinking)
                .with_output_limit(config.max_tokens)
                .with_temperature(config.temperature),
        )),
        "ollama" => Ok(Box::new(
            OpenAiProvider::new(base_url, ollama_api_key(config))
                .with_provider_name("ollama")
                .with_output_limit(config.max_tokens)
                .with_temperature(config.temperature),
        )),
        // NewAPI is a user-hosted OpenAI-compatible gateway.
        "newapi" => Ok(Box::new(
            OpenAiProvider::new(base_url, require_api_key(config)?)
                .with_provider_name("newapi")
                .with_reasoning_effort(config.reasoning_effort.clone())
                .with_reasoning_model(config.reasoning_model)
                .with_output_limit(config.max_tokens)
                .with_temperature(config.temperature),
        )),
        "anthropic" => Ok(Box::new(AnthropicProvider::new(
            base_url,
            require_api_key(config)?,
        ))),
        other => Err(unknown_provider_error(other)),
    }
}

/// 解析实际使用的 API 地址：配置的 `base_url` 优先，否则取该 provider 的默认地址。
fn resolve_base_url(config: &ModelConfig) -> Result<String> {
    if let Some(url) = &config.base_url {
        return Ok(url.clone());
    }
    if config.provider == "newapi" {
        return Err(XaimError::Config(
            "provider 'newapi' 必须配置 Base URL，例如 https://your-host/v1".into(),
        ));
    }
    default_base_url(&config.provider)
        .map(str::to_string)
        .ok_or_else(|| unknown_provider_error(&config.provider))
}

/// provider 标识 → 默认 API 地址；未知标识返回 `None`。
fn default_base_url(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some(OPENAI_BASE_URL),
        "deepseek" => Some(DEEPSEEK_BASE_URL),
        "qwen" => Some(QWEN_BASE_URL),
        "ollama" => Some(OLLAMA_BASE_URL),
        "anthropic" => Some(ANTHROPIC_BASE_URL),
        // NewAPI deployments are user-hosted; a base URL is required.
        "newapi" => None,
        _ => None,
    }
}

/// 云端 provider 必须配置 api key。
fn require_api_key(config: &ModelConfig) -> Result<String> {
    if config.api_key.trim().is_empty() {
        return Err(XaimError::Config(format!(
            "provider '{}' 缺少 api key，请在配置中填写",
            config.provider
        )));
    }
    Ok(config.api_key.clone())
}

/// Ollama 未配置 key 时用占位符补位。
fn ollama_api_key(config: &ModelConfig) -> String {
    if config.api_key.trim().is_empty() {
        OLLAMA_PLACEHOLDER_KEY.to_string()
    } else {
        config.api_key.clone()
    }
}

fn unknown_provider_error(provider: &str) -> XaimError {
    XaimError::Config(format!(
        "未知的模型提供方 '{provider}'，支持: {}",
        SUPPORTED_PROVIDERS.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_config(provider: &str, api_key: &str) -> ModelConfig {
        ModelConfig {
            provider: provider.to_string(),
            api_key: api_key.to_string(),
            base_url: None,
            reasoning_effort: None,
            reasoning_model: false,
            thinking: None,
            max_tokens: None,
            temperature: None,
        }
    }

    #[test]
    fn default_base_url_maps_each_provider() {
        assert_eq!(
            default_base_url("openai"),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            default_base_url("deepseek"),
            Some("https://api.deepseek.com/v1")
        );
        assert_eq!(
            default_base_url("qwen"),
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1")
        );
        assert_eq!(
            default_base_url("ollama"),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(
            default_base_url("anthropic"),
            Some("https://api.anthropic.com/v1")
        );
        assert_eq!(default_base_url("gemini"), None);
    }

    #[test]
    fn base_url_override_wins_over_default() {
        let mut config = model_config("deepseek", "key");
        config.base_url = Some("http://127.0.0.1:9000/v1/".to_string());
        assert_eq!(
            resolve_base_url(&config).unwrap(),
            "http://127.0.0.1:9000/v1/"
        );
    }

    #[test]
    fn creates_each_supported_provider() {
        for name in ["openai", "deepseek", "qwen", "anthropic"] {
            let provider = create_provider(&model_config(name, "sk-test")).unwrap();
            assert_eq!(provider.name(), name, "{name} 创建后标识不符");
        }
        // Ollama 无需 api key
        let provider = create_provider(&model_config("ollama", "")).unwrap();
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn cloud_providers_require_api_key() {
        for name in ["openai", "deepseek", "qwen", "anthropic"] {
            let err = create_provider(&model_config(name, "   "))
                .err()
                .expect("缺少 api key 应返回错误");
            let msg = err.to_string();
            assert!(
                msg.contains("api key") && msg.contains(name),
                "{name} 的错误信息不清晰: {msg}"
            );
        }
        let mut config = model_config("newapi", "   ");
        config.base_url = Some("https://example.test/v1".into());
        let msg = create_provider(&config)
            .err()
            .expect("NewAPI 缺少 api key 应返回错误")
            .to_string();
        assert!(msg.contains("api key") && msg.contains("newapi"), "{msg}");
    }

    #[test]
    fn newapi_requires_a_custom_base_url() {
        let err = create_provider(&model_config("newapi", "sk-test"))
            .err()
            .expect("NewAPI 没有地址时不能创建 provider");
        assert!(err.to_string().contains("Base URL"));
        let mut config = model_config("newapi", "Bearer sk-test");
        config.base_url = Some("https://example.test/newapi/v1".into());
        assert_eq!(create_provider(&config).unwrap().name(), "newapi");
    }

    #[test]
    fn newapi_passes_explicit_reasoning_options_and_dynamic_output_budget() {
        let mut config = model_config("newapi", "test-key");
        config.base_url = Some("https://example.test/v1".into());
        config.max_tokens = Some(128_000);
        config.reasoning_model = true;
        config.reasoning_effort = Some("low".into());
        let provider = create_provider(&config).unwrap();
        let request = provider::ToolRequest {
            model: "custom-model-id".into(),
            messages: vec![provider::ToolMessage::text("user", "Create HTML")],
            tools: vec![],
            max_tokens: 8192,
        };
        let metadata = provider.tool_request_metadata(&request);
        assert_eq!(metadata["reasoningEffort"], "low");
        assert_eq!(metadata["maxCompletionTokens"], 8192);
        assert!(metadata["maxTokens"].is_null());
        assert!(metadata["temperature"].is_null());
    }

    #[test]
    fn ollama_key_placeholder_and_passthrough() {
        assert_eq!(ollama_api_key(&model_config("ollama", "")), "ollama");
        assert_eq!(ollama_api_key(&model_config("ollama", "secret")), "secret");
    }

    #[test]
    fn unknown_provider_error_lists_supported_names() {
        let err = create_provider(&model_config("gemini", "k"))
            .err()
            .expect("未知 provider 应返回错误");
        let msg = err.to_string();
        assert!(msg.contains("gemini"), "实际错误: {msg}");
        for name in SUPPORTED_PROVIDERS {
            assert!(msg.contains(name), "错误应列出支持的 provider: {msg}");
        }
    }
}
