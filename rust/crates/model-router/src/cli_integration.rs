//! Integration helpers for wiring the model router into a CLI application.
//!
//! Provides utility functions to extract router configuration from merged
//! settings JSON, build orchestrators, and display cost comparison info.

use crate::config::RouterConfig;
use crate::orchestrator::{ClientFactory, Orchestrator};

/// Attempt to read a `RouterConfig` from the runtime config's merged settings.
/// Returns `None` when the `model_router` key is absent or invalid.
#[must_use]
pub fn router_config_from_settings(settings: &serde_json::Value) -> Option<RouterConfig> {
    RouterConfig::from_settings(settings)
}

/// Build an orchestrator if the model router is enabled in settings.
/// Returns `None` when not configured or disabled.
pub fn build_orchestrator(
    settings: &serde_json::Value,
    client_factory: Box<dyn ClientFactory>,
    system_prompt: Vec<String>,
) -> Option<Orchestrator> {
    let router_config = router_config_from_settings(settings)?;
    if !router_config.enabled {
        return None;
    }
    Some(Orchestrator::new(
        router_config,
        client_factory,
        system_prompt,
    ))
}

/// Print a cost comparison showing savings from model routing.
#[must_use]
pub fn cost_comparison_summary(config: &RouterConfig, task_description: &str) -> String {
    let multiplier = config.estimated_cost_multiplier();
    let savings_pct = ((1.0 - multiplier) * 100.0).round();
    let strategy_name = config.strategy.as_str();

    format!(
        "Model router: {} strategy\n  Planner: {}\n  Executor: {}\n  Verifier: {}\n  Estimated savings: {:.0}% vs using planner for everything\n  Task: {}",
        strategy_name,
        config.planner_model,
        config.executor_model,
        config.verifier_model,
        savings_pct,
        task_description,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_config_from_json_settings() {
        let json = serde_json::json!({
            "model_router": {
                "enabled": true,
                "strategy": "plan_only",
                "planner_model": "claude-sonnet-4-6",
                "executor_model": "claude-haiku-4-5-20251213"
            }
        });
        let config = RouterConfig::from_settings(&json).expect("should parse");
        assert!(config.enabled);
        assert_eq!(config.strategy, crate::strategy::RoutingStrategy::PlanOnly);
    }

    #[test]
    fn cost_summary_shows_models_and_savings() {
        let config = RouterConfig::default();
        let summary = cost_comparison_summary(&config, "Implement auth middleware");
        assert!(summary.contains("claude-opus-4-6"));
        assert!(summary.contains("claude-haiku-4-5-20251213"));
        assert!(summary.contains("plan_execute_verify"));
        assert!(summary.contains("Implement auth middleware"));
    }

    #[test]
    fn build_orchestrator_returns_none_when_disabled() {
        let json = serde_json::json!({
            "model_router": {
                "enabled": false
            }
        });
        // Use a type that implements ClientFactory via the blanket impl
        let factory: Box<dyn ClientFactory> =
            Box::new(|_: &str| -> Result<Box<dyn runtime::ApiClient>, String> {
                Err("not used".into())
            });
        let result = build_orchestrator(&json, factory, vec![]);
        assert!(result.is_none());
    }
}
