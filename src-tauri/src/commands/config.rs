//! 配置相关命令：读取与保存应用配置（模型接入 + 任务路由）。
//!
//! 持久化在 [`crate::config::model`]（SQLite `cache` 表），保存前校验。

use tauri::State;

use crate::errors::XaimError;
use crate::state::AppState;
use crate::types::Config;

/// 读取应用配置；从未保存过时返回默认值。
#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<Config, String> {
    Ok(crate::config::model::load(&state.store))
}

/// 保存应用配置（模型 provider 必须受支持）。
#[tauri::command]
pub async fn set_config(config: Config, state: State<'_, AppState>) -> Result<(), String> {
    set(&state, config).await.map_err(String::from)
}

/// `set_config` 的实现：校验通过才落库，失败时保留原配置。
pub(super) async fn set(state: &AppState, config: Config) -> Result<(), XaimError> {
    crate::config::model::validate(&config)?;
    crate::config::model::save(&state.store, &config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use crate::types::ModelConfig;

    fn test_state() -> AppState {
        AppState::with_store(Store::open_in_memory().unwrap())
    }

    #[tokio::test]
    async fn get_config_returns_default_before_any_save() {
        let state = test_state();
        let config = get_config_inner(&state).await;
        assert_eq!(config, Config::default());
        assert_eq!(config.model.provider, "deepseek");
    }

    async fn get_config_inner(state: &AppState) -> Config {
        crate::config::model::load(&state.store)
    }

    #[tokio::test]
    async fn set_config_persists_and_roundtrips() {
        let state = test_state();
        let config = Config {
            model: ModelConfig {
                protocol: crate::types::ApiProtocol::default(),
                provider: "ollama".to_string(),
                name: "qwen2.5-coder".to_string(),
                api_key: None,
                base_url: Some("http://127.0.0.1:11434/v1".to_string()),
                temperature: 0.4,
                max_tokens: Some(4096),
            },
            router: crate::model::router::RouterConfig {
                strong_model: "deepseek-r1".to_string(),
                ..crate::model::router::RouterConfig::default()
            },
            ..Config::default()
        };

        set(&state, config.clone()).await.unwrap();
        assert_eq!(get_config_inner(&state).await, config);
    }

    #[tokio::test]
    async fn set_config_rejects_unknown_provider_and_keeps_old_value() {
        let state = test_state();
        let original = Config::default();
        set(&state, original.clone()).await.unwrap();

        let mut bad = Config::default();
        bad.model.provider = "gemini".to_string();
        let err = set(&state, bad).await.unwrap_err();
        assert!(err.to_string().contains("gemini"), "{err}");

        assert_eq!(
            get_config_inner(&state).await,
            original,
            "校验失败不应改动已保存的配置"
        );
    }
}
