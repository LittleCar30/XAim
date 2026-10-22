//! 模型相关命令：读写模型配置、测试连接与列出可用模型。
//!
//! 配置持久化在 [`crate::config::model`]（SQLite `cache` 表），
//! 连接建立复用 [`crate::model::create_provider`]，命令层保持薄封装。

use std::time::Duration;

use tauri::State;

use crate::config;
use crate::errors::XaimError;
use crate::model::provider::{CompletionRequest, Message};
use crate::model::{self};
use crate::state::AppState;
use crate::types::ModelConfig;

/// 连接测试的超时上限：挂起的服务不应拖死配置界面。
const TEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 读取当前模型接入配置。
#[tauri::command]
pub async fn get_model_config(state: State<'_, AppState>) -> Result<ModelConfig, String> {
    Ok(config::model::load(&state.store).model)
}

/// 保存模型接入配置（provider 必须受支持）。
#[tauri::command]
pub async fn set_model_config(
    config: ModelConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    set_model(&state, config).await.map_err(String::from)
}

/// 测试模型连接：用给定配置发一条最小补全请求。
///
/// 返回值语义：`Err` = 配置无法发起测试（provider 未知、缺 api key）；
/// `Ok(false)` = 连接失败（网络、鉴权、模型不存在，原因记日志）；
/// `Ok(true)` = 连接成功。
#[tauri::command]
pub async fn test_model_connection(
    config: ModelConfig,
    reasoning_effort: Option<String>,
    thinking: Option<bool>,
    supports_reasoning: Option<bool>,
) -> Result<bool, String> {
    test_connection_with_options(
        config,
        reasoning_effort,
        thinking,
        supports_reasoning.unwrap_or(false),
    )
    .await
    .map_err(String::from)
}

/// 列出某 provider 的建议模型名（配置界面的候选清单，非完整枚举）。
#[tauri::command]
pub async fn list_models(provider: String) -> Result<Vec<String>, String> {
    catalog(&provider).ok_or_else(|| {
        XaimError::Config(format!(
            "未知的模型提供方 '{provider}'，支持: {}",
            model::SUPPORTED_PROVIDERS.join(", ")
        ))
        .to_string()
    })
}

/// 列出支持的模型提供方标识。
#[tauri::command]
pub async fn list_providers() -> Result<Vec<String>, String> {
    Ok(model::SUPPORTED_PROVIDERS
        .iter()
        .map(|name| name.to_string())
        .collect())
}

/// 保存当前模型，并同步对应 profile 和任务路由。
async fn set_model(state: &AppState, model: ModelConfig) -> Result<(), XaimError> {
    let mut app = config::model::load(&state.store);
    app.model = model;
    if let Some(profile) = app
        .model_profiles
        .iter_mut()
        .find(|p| Some(&p.id) == app.active_model_id.as_ref())
    {
        profile.config = app.model.clone();
        let efforts = model::capabilities::reasoning_efforts(&app.model.provider, &app.model.name);
        if !matches!(app.model.provider.as_str(), "openai" | "newapi") {
            profile.supports_reasoning = false;
        }
        if profile.reasoning_effort.as_ref().is_some_and(|v| {
            !efforts.contains(&v.as_str())
                || (matches!(app.model.provider.as_str(), "openai" | "newapi")
                    && !profile.supports_reasoning)
        }) {
            profile.reasoning_effort = None;
        }
        if !model::capabilities::supports_thinking_toggle(&app.model.provider, &app.model.name) {
            profile.thinking = None;
        }
    }
    app.router.fast_model = app.model.name.clone();
    app.router.strong_model = app.model.name.clone();
    app.router.overrides.clear();
    config::model::validate(&app)?;
    config::model::save(&state.store, &app)
}

/// `test_model_connection` 的实现：建 provider → 发最小请求 → 限时等待。
#[cfg(test)]
async fn test_connection(config: ModelConfig) -> Result<bool, XaimError> {
    test_connection_with_effort(config, None).await
}

#[cfg(test)]
async fn test_connection_with_effort(
    config: ModelConfig,
    effort: Option<String>,
) -> Result<bool, XaimError> {
    test_connection_with_options(config, effort, None, false).await
}

async fn test_connection_with_options(
    config: ModelConfig,
    effort: Option<String>,
    thinking: Option<bool>,
    supports_reasoning: bool,
) -> Result<bool, XaimError> {
    config::model::validate_sampling(&config)?;
    if supports_reasoning && !matches!(config.provider.as_str(), "openai" | "newapi") {
        return Err(XaimError::Config(
            "此提供方不支持 OpenAI 思考强度配置".into(),
        ));
    }
    if let Some(value) = &effort {
        if !model::capabilities::reasoning_efforts(&config.provider, &config.name)
            .contains(&value.as_str())
            || thinking == Some(false)
        {
            return Err(XaimError::Config("无效的思考强度".into()));
        }
    }
    if thinking.is_some()
        && !model::capabilities::supports_thinking_toggle(&config.provider, &config.name)
    {
        return Err(XaimError::Config("此模型尚未适配思考开关".into()));
    }
    let mut connection = config::model::ModelConfig::from(config.clone());
    connection.reasoning_effort = effort.clone();
    connection.reasoning_model = supports_reasoning || effort.is_some();
    connection.thinking = thinking;
    let provider = model::create_provider(&connection)?;

    let mut request = CompletionRequest::new(config.name, vec![Message::user("ping")]);
    request.max_tokens = if connection.reasoning_model
        || thinking == Some(true)
        || matches!(connection.provider.as_str(), "deepseek" | "qwen")
    {
        1024
    } else {
        8
    };
    match tokio::time::timeout(TEST_TIMEOUT, provider.complete(request)).await {
        Ok(Ok(_)) => Ok(true),
        Ok(Err(err)) => {
            tracing::warn!(provider = %connection.provider, error = %err, "模型连接测试失败");
            Ok(false)
        }
        Err(_) => {
            tracing::warn!(provider = %connection.provider, "模型连接测试超时");
            Ok(false)
        }
    }
}

/// 各 provider 的建议模型清单。
fn catalog(provider: &str) -> Option<Vec<String>> {
    let models: &[&str] = match provider {
        "openai" => &["gpt-4o", "gpt-4o-mini", "gpt-4.1", "o3-mini"],
        "deepseek" => &[
            "deepseek-flash",
            "deepseek-v4-pro",
            "deepseek-chat",
            "deepseek-reasoner",
        ],
        "qwen" => &["qwen-max", "qwen-plus", "qwen-turbo"],
        "ollama" => &["qwen2.5-coder", "deepseek-r1", "llama3.1", "codellama"],
        "anthropic" => &["claude-sonnet-4-5", "claude-opus-4-1", "claude-haiku-4-5"],
        // NewAPI model names are deployment-specific; these are only suggestions.
        "newapi" => &["deepseek-chat", "gpt-4o", "claude-sonnet-4-5"],
        _ => return None,
    };
    Some(models.iter().map(|name| name.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 内存库 + 空存储的测试状态。
    fn test_state() -> AppState {
        AppState::with_store(crate::store::Store::open_in_memory().unwrap())
    }

    /// 构造指向 mock 服务的 OpenAI 兼容模型配置。
    fn mock_config(url: &str, api_key: &str) -> ModelConfig {
        ModelConfig {
            protocol: crate::types::ApiProtocol::default(),
            provider: "deepseek".to_string(),
            name: "deepseek-chat".to_string(),
            api_key: Some(api_key.to_string()),
            base_url: Some(url.to_string()),
            temperature: 0.7,
            max_tokens: None,
        }
    }

    #[tokio::test]
    async fn connection_probe_uses_selected_responses_protocol() {
        let mut server = mockito::Server::new_async().await;
        let mock=server.mock("POST","/responses")
            .match_body(mockito::Matcher::PartialJson(json!({"model":"deepseek-flash","stream":false,"max_output_tokens":1024,"reasoning":{"effort":"none"}})))
            .with_header("content-type","application/json")
            .with_body(json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"pong"}]}]}).to_string())
            .create_async().await;
        let mut config = mock_config(&server.url(), "test-key");
        config.protocol = crate::types::ApiProtocol::Responses;
        config.name = "deepseek-flash".into();
        assert!(
            test_connection_with_options(config, None, Some(false), false)
                .await
                .unwrap()
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn connection_probe_preserves_default_reasoning_and_saved_temperature() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Json(json!({"model":"custom-reasoner","messages":[{"role":"user","content":"ping"}],"max_completion_tokens":1024,"stream":false})))
            .with_status(200).with_body(r#"{"choices":[{"finish_reason":"stop","message":{"content":"pong"}}]}"#)
            .create_async().await;
        let mut config = mock_config(&server.url(), "test-key");
        config.provider = "openai".into();
        config.name = "custom-reasoner".into();
        config.max_tokens = Some(65_536);
        assert!(test_connection_with_options(config, None, None, true)
            .await
            .unwrap());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn legacy_model_save_preserves_gateway_reasoning_and_updates_profile() {
        let state = test_state();
        let gateway = ModelConfig {
            provider: "newapi".into(),
            name: "reasoner".into(),
            base_url: Some("https://example.test/v1".into()),
            ..ModelConfig::default()
        };
        let mut app = crate::types::Config {
            model: gateway.clone(),
            active_model_id: Some("gateway".into()),
            ..Default::default()
        };
        app.model_profiles.push(crate::types::ModelProfile {
            id: "gateway".into(),
            label: "Gateway".into(),
            config: gateway.clone(),
            supports_reasoning: true,
            reasoning_effort: Some("high".into()),
            thinking: None,
        });
        config::model::save(&state.store, &app).unwrap();
        let edited = ModelConfig {
            temperature: 0.25,
            ..gateway
        };
        set_model(&state, edited.clone()).await.unwrap();
        let saved = config::model::load(&state.store);
        assert_eq!(saved.model, edited);
        assert_eq!(saved.model_profiles[0].config, edited);
        assert!(saved.model_profiles[0].supports_reasoning);
        assert_eq!(
            saved.model_profiles[0].reasoning_effort.as_deref(),
            Some("high")
        );
        assert_eq!(saved.router.fast_model, "reasoner");
    }

    /// 直接读存储（绕过命令包装便于断言）。
    async fn current_model(state: &AppState) -> ModelConfig {
        config::model::load(&state.store).model
    }

    #[tokio::test]
    async fn set_then_get_roundtrips_model_config() {
        let state = test_state();
        let model = ModelConfig {
            provider: "openai".to_string(),
            name: "gpt-4o".to_string(),
            api_key: Some("sk-test".to_string()),
            temperature: 0.2,
            max_tokens: Some(2048),
            ..ModelConfig::default()
        };

        set_model(&state, model.clone()).await.unwrap();
        assert_eq!(current_model(&state).await, model);

        // 所有执行阶段都使用当前所选模型。
        let app = config::model::load(&state.store);
        assert_eq!(app.router.fast_model, "gpt-4o");
        assert_eq!(app.router.strong_model, "gpt-4o");
    }

    #[tokio::test]
    async fn set_model_rejects_unknown_provider_without_persisting() {
        let state = test_state();
        let model = ModelConfig {
            provider: "gemini".to_string(),
            ..ModelConfig::default()
        };

        let err = set_model(&state, model).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("gemini") && msg.contains("deepseek"), "{msg}");
        // 校验失败不应落库。
        assert_eq!(current_model(&state).await, ModelConfig::default());
    }

    #[tokio::test]
    async fn connection_test_succeeds_against_openai_compatible_mock() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_header("authorization", "Bearer test-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "choices": [{"message": {"role": "assistant", "content": "pong"}}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1}
                })
                .to_string(),
            )
            .create_async()
            .await;

        let ok = test_connection(mock_config(&server.url(), "test-key"))
            .await
            .unwrap();
        assert!(ok, "正常响应应判定连接成功");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn connection_test_reports_auth_failure_as_false() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(401)
            .with_body(r#"{"error":{"message":"invalid api key"}}"#)
            .create_async()
            .await;

        let ok = test_connection(mock_config(&server.url(), "bad-key"))
            .await
            .unwrap();
        assert!(!ok, "鉴权失败应判定连接不成功");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn connection_test_fails_fast_on_bad_config() {
        // 云端 provider 缺 api key：无法发起测试，返回 Err。
        let config = ModelConfig {
            api_key: None,
            ..ModelConfig::default()
        };
        let err = test_connection(config).await.unwrap_err();
        assert!(err.to_string().contains("api key"), "{err}");

        // 未知的 provider。
        let config = ModelConfig {
            provider: "gemini".to_string(),
            api_key: Some("k".to_string()),
            ..ModelConfig::default()
        };
        let err = test_connection(config).await.unwrap_err();
        assert!(err.to_string().contains("gemini"), "{err}");
    }

    #[tokio::test]
    async fn connection_test_rejects_refused_endpoint_as_false() {
        // 占用一个端口后立即释放：连接大概率被拒。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let ok = test_connection(mock_config(&format!("http://{addr}/v1"), "k"))
            .await
            .unwrap();
        assert!(!ok, "连接拒绝应判定为不成功而非报错");
    }

    #[tokio::test]
    async fn list_models_covers_known_providers_and_rejects_unknown() {
        for provider in model::SUPPORTED_PROVIDERS {
            let models = catalog(provider).expect("支持的 provider 应有模型清单");
            assert!(!models.is_empty(), "{provider} 的模型清单不应为空");
        }
        assert!(catalog("gemini").is_none());

        // 命令层的未知 provider 报错列出支持清单。
        let err = list_models("gemini".to_string()).await.unwrap_err();
        assert!(err.contains("gemini") && err.contains("anthropic"), "{err}");
    }

    #[tokio::test]
    async fn list_providers_returns_supported_list() {
        let providers = list_providers().await.unwrap();
        assert_eq!(
            providers,
            model::SUPPORTED_PROVIDERS
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        );
    }
}
