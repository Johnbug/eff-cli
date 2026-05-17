use serde::{Deserialize, Serialize};

use crate::strategy::RoutingStrategy;

/// Configuration for the model router — which model to use for each
/// phase of the cost-optimized pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterConfig {
    /// Model used for the planning phase (smart, expensive).
    #[serde(default = "RouterConfig::default_planner")]
    pub planner_model: String,

    /// Model used for the execution phase (cheaper, faster).
    #[serde(default = "RouterConfig::default_executor")]
    pub executor_model: String,

    /// Model used for the verification phase (smart, accurate).
    /// Defaults to the planner model when not set.
    #[serde(default = "RouterConfig::default_verifier")]
    pub verifier_model: String,

    /// The routing strategy to use.
    #[serde(default)]
    pub strategy: RoutingStrategy,

    /// Whether the model router is enabled. When false, all phases
    /// use the default model set in the runtime config.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            planner_model: Self::default_planner(),
            executor_model: Self::default_executor(),
            verifier_model: Self::default_verifier(),
            strategy: RoutingStrategy::default(),
            enabled: true,
        }
    }
}

impl RouterConfig {
    #[must_use]
    pub fn default_planner() -> String {
        "claude-opus-4-6".to_string()
    }

    #[must_use]
    pub fn default_executor() -> String {
        "claude-haiku-4-5-20251213".to_string()
    }

    #[must_use]
    pub fn default_verifier() -> String {
        // By default, use the planner model for verification too
        Self::default_planner()
    }

    /// Resolve which model to use for a given tier.
    #[must_use]
    pub fn model_for_tier(&self, tier: crate::tiers::ModelTier) -> &str {
        match tier {
            crate::tiers::ModelTier::Planner => &self.planner_model,
            crate::tiers::ModelTier::Executor => &self.executor_model,
            crate::tiers::ModelTier::Verifier => &self.verifier_model,
        }
    }

    /// Returns estimated cost multiplier vs using planner for everything.
    /// Lower is better. For example, 0.3 means ~70% cost savings.
    #[must_use]
    pub fn estimated_cost_multiplier(&self) -> f64 {
        // Rough estimate based on typical pricing ratios
        let planner_weight = 0.15; // planning is ~15% of total tokens
        let executor_weight = 0.75; // execution is ~75% of total tokens
        let verifier_weight = 0.10; // verification is ~10% of total tokens

        let planner_cost = model_cost_ratio(&self.planner_model);
        let executor_cost = model_cost_ratio(&self.executor_model);
        let verifier_cost = model_cost_ratio(&self.verifier_model);

        let blended = planner_weight * planner_cost
            + executor_weight * executor_cost
            + verifier_weight * verifier_cost;

        // Normalize against using planner for everything
        blended / planner_cost.max(0.001)
    }
}

/// Rough cost ratio relative to Opus (priced at 1.0).
fn model_cost_ratio(model: &str) -> f64 {
    let lower = model.to_ascii_lowercase();
    if lower.contains("haiku") {
        0.067 // Haiku is ~15x cheaper than Opus
    } else if lower.contains("sonnet") || lower.contains("opus") {
        1.0
    } else if lower.contains("gpt-4.1-mini") || lower.contains("gpt-4.1-nano") {
        0.04
    } else if lower.contains("gpt-4.1") {
        0.13
    } else if lower.contains("gpt-5.4-mini") || lower.contains("gpt-5.4-nano") {
        0.02
    } else if lower.contains("gpt-5.4") {
        0.07
    } else if lower.contains("grok-mini") || lower.contains("grok-3-mini") {
        0.06
    } else if lower.contains("grok") {
        0.33
    } else {
        1.0 // unknown, assume same as Opus
    }
}

impl RouterConfig {
    /// Extract `RouterConfig` from a merged settings JSON value.
    ///
    /// Looks for a `model_router` key in the settings object.
    /// Returns `None` if the key is absent or parsing fails.
    #[must_use]
    pub fn from_settings(settings: &serde_json::Value) -> Option<Self> {
        let obj = settings.as_object()?;
        let router_value = obj.get("model_router")?;
        serde_json::from_value(router_value.clone()).ok()
    }

    /// Extract `RouterConfig` from a raw settings JSON string.
    #[must_use]
    pub fn from_settings_json(json: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        Self::from_settings(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_routes_tiers_correctly() {
        let config = RouterConfig::default();
        assert_eq!(
            config.model_for_tier(crate::tiers::ModelTier::Planner),
            "claude-opus-4-6"
        );
        assert_eq!(
            config.model_for_tier(crate::tiers::ModelTier::Executor),
            "claude-haiku-4-5-20251213"
        );
        assert_eq!(
            config.model_for_tier(crate::tiers::ModelTier::Verifier),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn cost_multiplier_shows_savings_with_haiku_executor() {
        let config = RouterConfig::default();
        let multiplier = config.estimated_cost_multiplier();
        // With Haiku as executor (75% of tokens, 6.7% cost), should be ~30% of full cost
        assert!(
            multiplier < 0.5,
            "expected significant savings, got {multiplier}"
        );
    }

    #[test]
    fn no_savings_when_all_opus() {
        let config = RouterConfig {
            planner_model: "claude-opus-4-6".into(),
            executor_model: "claude-opus-4-6".into(),
            verifier_model: "claude-opus-4-6".into(),
            ..Default::default()
        };
        let multiplier = config.estimated_cost_multiplier();
        assert!(
            (multiplier - 1.0).abs() < 0.01,
            "expected ~1.0, got {multiplier}"
        );
    }

    #[test]
    fn from_settings_parses_model_router_key() {
        let json = serde_json::json!({
            "model_router": {
                "enabled": true,
                "strategy": "plan_execute_verify",
                "planner_model": "claude-opus-4-6",
                "executor_model": "claude-haiku-4-5-20251213",
                "verifier_model": "claude-sonnet-4-6"
            },
            "other": "ignored"
        });
        let config = RouterConfig::from_settings(&json).expect("should parse config");
        assert!(config.enabled);
        assert_eq!(config.planner_model, "claude-opus-4-6");
        assert_eq!(config.executor_model, "claude-haiku-4-5-20251213");
        assert_eq!(config.strategy, RoutingStrategy::PlanExecuteVerify);
    }

    #[test]
    fn from_settings_returns_none_when_key_absent() {
        let json = serde_json::json!({
            "model": "sonnet",
            "other": "value"
        });
        assert!(RouterConfig::from_settings(&json).is_none());
    }

    #[test]
    fn from_settings_uses_defaults_for_partial_config() {
        let json = serde_json::json!({
            "model_router": {
                "executor_model": "grok-mini"
            }
        });
        let config = RouterConfig::from_settings(&json).expect("should parse partial config");
        assert_eq!(config.executor_model, "grok-mini");
        // defaults for unspecified fields
        assert_eq!(config.planner_model, RouterConfig::default_planner());
        assert_eq!(config.strategy, RoutingStrategy::default());
    }

    #[test]
    fn from_settings_json_parses_string() {
        let config = RouterConfig::from_settings_json(
            r#"{"model_router":{"enabled":false,"strategy":"plan_only"}}"#,
        )
        .expect("should parse JSON string");
        assert!(!config.enabled);
        assert_eq!(config.strategy, RoutingStrategy::PlanOnly);
    }

    #[test]
    fn model_ratio_estimates() {
        // Haiku is ~15x cheaper than Opus
        let ratio = super::model_cost_ratio("claude-haiku-4-5-20251213");
        assert!((ratio - 0.067).abs() < 0.001, "got {ratio}");

        // GPT small models are very cheap
        let nano_ratio = super::model_cost_ratio("gpt-4.1-nano");
        assert!(nano_ratio < 0.1, "got {nano_ratio}");
    }
}
