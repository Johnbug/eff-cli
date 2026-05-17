use serde::{Deserialize, Serialize};

/// The role a model plays in the cost-optimized pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    /// Smart/expensive model used for planning and analysis.
    Planner,
    /// Cheaper model used for executing planned steps.
    Executor,
    /// Smart model used to verify and review results.
    Verifier,
}

impl ModelTier {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Executor => "executor",
            Self::Verifier => "verifier",
        }
    }
}

impl std::fmt::Display for ModelTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
