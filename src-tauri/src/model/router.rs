//! 模型路由：按任务类型在 Fast / Strong 两档模型间选择。
//!
//! 默认策略把轻量任务（意图解析、检索重排、总结）分给 Fast 档，
//! 把重度任务（代码生成、修复）分给 Strong 档；[`RouterConfig::overrides`]
//! 可按任务覆盖档位与模型名。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::model::AppConfig;

/// 任务类型，路由的输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    /// 意图解析。
    Intent,
    /// 检索重排。
    Retrieve,
    /// 代码生成。
    Generate,
    /// 修复。
    Repair,
    /// 总结。
    Summarize,
}

/// 选中的模型：`Fast` 档偏速度与成本，`Strong` 档偏能力，值为模型名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelChoice {
    Fast(String),
    Strong(String),
}

impl ModelChoice {
    /// 模型名（不含档位），用于填 [`CompletionRequest::model`]。
    pub fn name(&self) -> &str {
        match self {
            ModelChoice::Fast(name) | ModelChoice::Strong(name) => name,
        }
    }

    /// 是否为 Fast 档。
    pub fn is_fast(&self) -> bool {
        matches!(self, ModelChoice::Fast(_))
    }

    /// 是否为 Strong 档。
    pub fn is_strong(&self) -> bool {
        matches!(self, ModelChoice::Strong(_))
    }
}

/// 路由配置：两档各自的默认模型名，外加按任务的覆盖映射。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RouterConfig {
    /// Fast 档默认模型名。
    pub fast_model: String,
    /// Strong 档默认模型名。
    pub strong_model: String,
    /// 按任务的覆盖映射：命中时整体取代默认档位（档位与模型名都以此为准）。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<TaskKind, ModelChoice>,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            // TODO: 接入配置界面后由用户填写，这里先给一组开箱可用的 DeepSeek 组合。
            fast_model: "deepseek-chat".to_string(),
            strong_model: "deepseek-reasoner".to_string(),
            overrides: BTreeMap::new(),
        }
    }
}

/// 任务档位（内部）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Fast,
    Strong,
}

/// 默认策略：轻量任务走 Fast 档，代码生成与修复走 Strong 档。
fn default_tier(kind: TaskKind) -> Tier {
    match kind {
        TaskKind::Intent | TaskKind::Retrieve | TaskKind::Summarize => Tier::Fast,
        TaskKind::Generate | TaskKind::Repair => Tier::Strong,
    }
}

/// 按任务类型选模型：先查配置里的覆盖映射，未命中时按默认档位
/// 取 `router.fast_model` / `router.strong_model`。
pub fn pick_model(kind: TaskKind, config: &AppConfig) -> ModelChoice {
    if let Some(choice) = config.router.overrides.get(&kind) {
        return choice.clone();
    }
    match default_tier(kind) {
        Tier::Fast => ModelChoice::Fast(config.router.fast_model.clone()),
        Tier::Strong => ModelChoice::Strong(config.router.strong_model.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_routes_each_task_kind() {
        let config = AppConfig::default();
        let cases = [
            (TaskKind::Intent, true),
            (TaskKind::Retrieve, true),
            (TaskKind::Summarize, true),
            (TaskKind::Generate, false),
            (TaskKind::Repair, false),
        ];
        for (kind, fast) in cases {
            let choice = pick_model(kind, &config);
            assert_eq!(
                choice.is_fast(),
                fast,
                "{kind:?} 应路由为 {}",
                if fast { "Fast" } else { "Strong" }
            );
        }
    }

    #[test]
    fn default_config_supplies_model_names() {
        let config = AppConfig::default();
        assert_eq!(
            pick_model(TaskKind::Intent, &config),
            ModelChoice::Fast("deepseek-chat".to_string())
        );
        assert_eq!(
            pick_model(TaskKind::Generate, &config),
            ModelChoice::Strong("deepseek-reasoner".to_string())
        );
    }

    #[test]
    fn override_wins_over_default_policy() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            TaskKind::Summarize,
            ModelChoice::Strong("big-summary-model".to_string()),
        );
        overrides.insert(
            TaskKind::Generate,
            ModelChoice::Fast("mini-coder".to_string()),
        );
        let config = AppConfig {
            router: RouterConfig {
                overrides,
                ..RouterConfig::default()
            },
            ..AppConfig::default()
        };

        assert_eq!(
            pick_model(TaskKind::Summarize, &config),
            ModelChoice::Strong("big-summary-model".to_string())
        );
        assert_eq!(
            pick_model(TaskKind::Generate, &config),
            ModelChoice::Fast("mini-coder".to_string())
        );
        // 未覆盖的任务仍走默认策略
        assert_eq!(
            pick_model(TaskKind::Repair, &config),
            ModelChoice::Strong("deepseek-reasoner".to_string())
        );
        assert_eq!(
            pick_model(TaskKind::Intent, &config),
            ModelChoice::Fast("deepseek-chat".to_string())
        );
    }

    #[test]
    fn configured_tier_models_are_respected() {
        let config = AppConfig {
            router: RouterConfig {
                fast_model: "gpt-4o-mini".to_string(),
                strong_model: "gpt-4o".to_string(),
                overrides: BTreeMap::new(),
            },
            ..AppConfig::default()
        };
        assert_eq!(
            pick_model(TaskKind::Retrieve, &config),
            ModelChoice::Fast("gpt-4o-mini".to_string())
        );
        assert_eq!(
            pick_model(TaskKind::Repair, &config),
            ModelChoice::Strong("gpt-4o".to_string())
        );
    }

    #[test]
    fn model_choice_accessors() {
        let fast = ModelChoice::Fast("a".to_string());
        let strong = ModelChoice::Strong("b".to_string());
        assert!(fast.is_fast() && !fast.is_strong());
        assert!(strong.is_strong() && !strong.is_fast());
        assert_eq!(fast.name(), "a");
        assert_eq!(strong.name(), "b");
    }

    #[test]
    fn router_config_roundtrips_through_json() {
        let mut overrides = BTreeMap::new();
        overrides.insert(TaskKind::Repair, ModelChoice::Fast("quick-fix".to_string()));
        let config = RouterConfig {
            overrides,
            ..RouterConfig::default()
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: RouterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn empty_json_falls_back_to_defaults() {
        let json = serde_json::to_string(&RouterConfig::default()).unwrap();
        assert!(!json.contains("overrides"), "空覆盖映射不应序列化: {json}");

        let parsed: RouterConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, RouterConfig::default());

        let partial: RouterConfig =
            serde_json::from_str(r#"{"fastModel": "gpt-4o-mini"}"#).unwrap();
        assert_eq!(partial.fast_model, "gpt-4o-mini");
        assert_eq!(partial.strong_model, "deepseek-reasoner");
        assert!(partial.overrides.is_empty());
    }
}
