//! Model Router — cost-optimized model routing for the claw CLI.
//!
//! This crate provides a configurable pipeline that routes different
//! phases of work to different models based on their cost and capability:
//!
//! - **Planner** (smart/expensive): Analyzes tasks and creates step-by-step plans.
//! - **Executor** (cheap/fast): Implements each step using available tools.
//! - **Verifier** (smart/accurate): Reviews results for correctness.
//!
//! # Configuration
//!
//! ```json
//! {
//!   "model_router": {
//!     "enabled": true,
//!     "strategy": "plan_execute_verify",
//!     "planner_model": "claude-opus-4-6",
//!     "executor_model": "claude-haiku-4-5-20251213",
//!     "verifier_model": "claude-sonnet-4-6"
//!   }
//! }
//! ```

#![forbid(unsafe_code)]

pub mod cli_integration;
pub mod config;
pub mod orchestrator;
pub mod plan;
pub mod strategy;
pub mod tiers;

pub use cli_integration::{
    build_orchestrator, cost_comparison_summary, router_config_from_settings,
};
pub use config::RouterConfig;
pub use orchestrator::{
    ClientFactory, Orchestrator, OrchestratorError, OrchestratorResult, PhaseResult,
};
pub use plan::{Plan, PlanStep};
pub use strategy::RoutingStrategy;
pub use tiers::ModelTier;
