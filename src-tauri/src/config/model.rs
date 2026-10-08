//! 配置模型与读写：IPC 契约 [`crate::types::Config`] 在 SQLite `cache` 表
//! 中的持久化（键 [`CONFIG_KEY`]，JSON 编码，无 TTL）。
//!
//! - [`load`] 缺失或损坏时回退默认值（损坏只记日志，不让应用起不来）；
//! - [`save`] 整体覆盖写入；
//! - [`validate`] 校验 provider 受支持，供 `set_config` / `set_model_config` 调用。

use serde::{Deserialize, Serialize};

use crate::errors::{Result, XaimError};
use crate::model::router::RouterConfig;
use crate::store::{cache, Store};
use crate::types::Config;

/// 配置在 `cache` 表中的键。
pub const CONFIG_KEY: &str = "app:config";

/// 模型提供方连接配置，供 [`crate::model::create_provider`] 使用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// 提供方标识：`openai` / `deepseek` / `qwen` / `ollama` / `anthropic`。
    pub provider: String,
    /// API key；Ollama 等本地服务可留空。
    pub api_key: String,
    /// 自定义 API 地址（代理、私有化部署）；留空用该 provider 的默认地址。
    pub base_url: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_model: bool,
    /// Explicit output budget from the saved model settings.
    pub max_tokens: Option<u32>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            // TODO: 与 router 的默认模型保持一致，接入配置界面后由用户填写。
            provider: "deepseek".to_string(),
            api_key: String::new(),
            base_url: None,
            reasoning_effort: None,
            reasoning_model: false,
            max_tokens: None,
        }
    }
}

impl From<crate::types::ModelConfig> for ModelConfig {
    /// 从 IPC 契约的模型配置提取连接参数（provider/key/地址）；
    /// 模型名、采样参数等不参与连接建立。
    fn from(config: crate::types::ModelConfig) -> Self {
        Self {
            provider: config.provider,
            api_key: config.api_key.unwrap_or_default(),
            base_url: config.base_url,
            reasoning_effort: None,
            reasoning_model: false,
            max_tokens: config.max_tokens,
        }
    }
}

/// 应用配置。TODO: 补充数据目录、密钥存储与界面偏好。
#[derive(Debug, Default)]
pub struct AppConfig {
    pub data_dir: String,
    /// 模型提供方连接配置。
    pub model: ModelConfig,
    /// 模型路由配置。
    pub router: RouterConfig,
}

/// 从存储加载配置；缺失或损坏时返回默认值（损坏记警告日志）。
pub fn load(store: &Store) -> Config {
    let value = {
        let conn = store.conn();
        cache::get_cache(&conn, CONFIG_KEY)
    };
    let Ok(Some(json)) = value else {
        return Config::default();
    };
    match serde_json::from_str(&json) {
        Ok(config) => config,
        Err(err) => {
            tracing::warn!(error = %err, "存储中的配置损坏，回退默认值");
            Config::default()
        }
    }
}

/// 把配置整体覆盖写入存储。
pub fn save(store: &Store, config: &Config) -> Result<()> {
    let json = serde_json::to_string(config)?;
    let conn = store.conn();
    cache::set_cache(&conn, CONFIG_KEY, &json, None)
}

/// 校验配置：模型提供方必须受支持。
pub fn validate(config: &Config) -> Result<()> {
    if !crate::model::SUPPORTED_PROVIDERS.contains(&config.model.provider.as_str()) {
        return Err(XaimError::Config(format!(
            "未知的模型提供方 '{}'，支持: {}",
            config.model.provider,
            crate::model::SUPPORTED_PROVIDERS.join(", ")
        )));
    }
    let mut ids = std::collections::HashSet::new();
    for profile in &config.model_profiles {
        if profile.supports_reasoning && profile.config.provider != "openai" {
            return Err(XaimError::Config("此提供方不支持 reasoning_effort".into()));
        }
        if profile.id.trim().is_empty()
            || !ids.insert(&profile.id)
            || profile.label.trim().is_empty()
            || profile.config.name.trim().is_empty()
        {
            return Err(XaimError::Config(
                "模型名称和标识不能为空，标识不可重复".into(),
            ));
        }
        if !crate::model::SUPPORTED_PROVIDERS.contains(&profile.config.provider.as_str()) {
            return Err(XaimError::Config("模型提供方不受支持".into()));
        }
        if let Some(effort) = &profile.reasoning_effort {
            if !profile.supports_reasoning
                || profile.config.provider != "openai"
                || !["low", "medium", "high"].contains(&effort.as_str())
            {
                return Err(XaimError::Config("此模型不支持指定的思考强度".into()));
            }
        }
    }
    if let Some(id) = &config.active_model_id {
        let profile = config
            .model_profiles
            .iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| XaimError::Config("所选模型不存在".into()))?;
        if profile.config != config.model {
            return Err(XaimError::Config("所选模型与当前连接配置不一致".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_roundtrip_and_validate_selection() {
        let store = Store::open_in_memory().unwrap();
        let mut config = Config::default();
        let profile = crate::types::ModelProfile {
            id: "reasoner".into(),
            label: "Reasoner".into(),
            config: crate::types::ModelConfig {
                provider: "openai".into(),
                name: "o3-mini".into(),
                ..Default::default()
            },
            supports_reasoning: true,
            reasoning_effort: Some("high".into()),
        };
        config.model = profile.config.clone();
        config.active_model_id = Some(profile.id.clone());
        config.model_profiles = vec![profile];
        validate(&config).unwrap();
        save(&store, &config).unwrap();
        assert_eq!(load(&store), config);
        config.active_model_id = Some("missing".into());
        assert!(validate(&config).is_err());
        config.active_model_id = Some("reasoner".into());
        config.model_profiles[0].reasoning_effort = Some("invalid".into());
        assert!(validate(&config).is_err());
    }

    #[test]
    fn model_config_roundtrips_through_json() {
        let config = ModelConfig {
            provider: "qwen".to_string(),
            api_key: "sk-xxx".to_string(),
            base_url: Some("http://127.0.0.1:9000/v1".to_string()),
            reasoning_effort: None,
            reasoning_model: false,
            max_tokens: Some(1024),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ModelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn empty_model_config_json_falls_back_to_defaults() {
        let parsed: ModelConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, ModelConfig::default());
        assert_eq!(parsed.provider, "deepseek");
        assert!(parsed.api_key.is_empty());
        assert_eq!(parsed.base_url, None);

        let partial: ModelConfig = serde_json::from_str(r#"{"provider": "ollama"}"#).unwrap();
        assert_eq!(partial.provider, "ollama");
        assert_eq!(partial.base_url, None);
    }

    #[test]
    fn connection_config_extracts_from_ipc_contract() {
        let ipc = crate::types::ModelConfig {
            provider: "openai".to_string(),
            name: "gpt-4o".to_string(),
            api_key: Some("sk-test".to_string()),
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            temperature: 0.3,
            max_tokens: Some(1024),
        };
        let connection = ModelConfig::from(ipc);
        assert_eq!(connection.max_tokens, Some(1024));
        assert_eq!(connection.provider, "openai");
        assert_eq!(connection.api_key, "sk-test");
        assert_eq!(
            connection.base_url.as_deref(),
            Some("http://127.0.0.1:9/v1")
        );

        // api key 缺省视为空串（Ollama 等本地服务可用）。
        let ipc = crate::types::ModelConfig::default();
        assert_eq!(ModelConfig::from(ipc).api_key, "");
    }

    #[test]
    fn save_then_load_roundtrips_config() {
        let store = Store::open_in_memory().unwrap();
        let config = Config {
            model: crate::types::ModelConfig {
                provider: "openai".to_string(),
                name: "gpt-4o".to_string(),
                api_key: Some("sk-test".to_string()),
                base_url: None,
                temperature: 0.2,
                max_tokens: Some(2048),
            },
            router: RouterConfig {
                fast_model: "gpt-4o-mini".to_string(),
                ..RouterConfig::default()
            },
            ..Config::default()
        };

        save(&store, &config).unwrap();
        assert_eq!(load(&store), config, "保存后应能原样读回");
    }

    #[test]
    fn load_without_saved_config_returns_default() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(load(&store), Config::default());
    }

    #[test]
    fn load_with_corrupt_json_falls_back_to_default() {
        let store = Store::open_in_memory().unwrap();
        {
            let conn = store.conn();
            cache::set_cache(&conn, CONFIG_KEY, "{ 不是 JSON", None).unwrap();
        }

        assert_eq!(load(&store), Config::default(), "损坏的配置应回退默认值");
        // 回退后可以正常覆盖保存。
        save(&store, &Config::default()).unwrap();
        assert_eq!(load(&store), Config::default());
    }

    #[test]
    fn validate_accepts_supported_and_rejects_unknown_providers() {
        let mut config = Config::default();
        for provider in crate::model::SUPPORTED_PROVIDERS {
            config.model.provider = provider.to_string();
            validate(&config).unwrap();
        }

        config.model.provider = "gemini".to_string();
        let err = validate(&config).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("gemini") && msg.contains("deepseek"), "{msg}");
    }
}
