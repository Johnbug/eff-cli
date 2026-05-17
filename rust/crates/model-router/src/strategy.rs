use serde::{Deserialize, Serialize};

/// The routing strategy determines how the pipeline distributes work
/// across model tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    /// Full pipeline: plan → execute → verify.
    /// - Planner produces a structured plan
    /// - Executor implements each step
    /// - Verifier reviews the result
    #[default]
    PlanExecuteVerify,

    /// Only route the planning phase through the smart model;
    /// everything else uses the executor model. No separate verification.
    PlanOnly,

    /// Smart model plans and the plan is broken into sub-tasks that are
    /// dispatched to independent executor subagents (future capability).
    PlanAndDispatch,

    /// Verification-only mode: use the default model for execution
    /// but run a verification pass with the verifier model afterward.
    VerifyOnly,
}

impl RoutingStrategy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlanExecuteVerify => "plan_execute_verify",
            Self::PlanOnly => "plan_only",
            Self::PlanAndDispatch => "plan_and_dispatch",
            Self::VerifyOnly => "verify_only",
        }
    }

    #[must_use]
    pub const fn has_planning_phase(self) -> bool {
        matches!(
            self,
            Self::PlanExecuteVerify | Self::PlanOnly | Self::PlanAndDispatch
        )
    }

    #[must_use]
    pub const fn has_execution_phase(self) -> bool {
        matches!(
            self,
            Self::PlanExecuteVerify | Self::PlanOnly | Self::PlanAndDispatch
        )
    }

    #[must_use]
    pub const fn has_verification_phase(self) -> bool {
        matches!(self, Self::PlanExecuteVerify | Self::VerifyOnly)
    }
}

impl std::fmt::Display for RoutingStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_strategy_is_plan_execute_verify() {
        assert_eq!(
            RoutingStrategy::default(),
            RoutingStrategy::PlanExecuteVerify
        );
    }

    #[test]
    fn plan_execute_verify_has_all_phases() {
        let s = RoutingStrategy::PlanExecuteVerify;
        assert!(s.has_planning_phase());
        assert!(s.has_execution_phase());
        assert!(s.has_verification_phase());
    }

    #[test]
    fn plan_only_skips_verification() {
        let s = RoutingStrategy::PlanOnly;
        assert!(s.has_planning_phase());
        assert!(s.has_execution_phase());
        assert!(!s.has_verification_phase());
    }

    #[test]
    fn verify_only_only_verifies() {
        let s = RoutingStrategy::VerifyOnly;
        assert!(!s.has_planning_phase());
        assert!(!s.has_execution_phase());
        assert!(s.has_verification_phase());
    }

    #[test]
    fn serialization_round_trip() {
        for strategy in &[
            RoutingStrategy::PlanExecuteVerify,
            RoutingStrategy::PlanOnly,
            RoutingStrategy::PlanAndDispatch,
            RoutingStrategy::VerifyOnly,
        ] {
            let json = serde_json::to_string(strategy).expect("serialize");
            let round_tripped: RoutingStrategy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*strategy, round_tripped);
        }
    }
}
