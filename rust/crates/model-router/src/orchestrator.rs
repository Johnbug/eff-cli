use runtime::{
    format_usd, ApiClient, ApiRequest, AssistantEvent, ContentBlock, ConversationMessage,
    TokenUsage,
};

use crate::config::RouterConfig;
use crate::plan::Plan;
#[cfg_attr(not(test), allow(unused_imports))]
use crate::strategy::RoutingStrategy;
use crate::tiers::ModelTier;

/// Result of a single phase in the pipeline.
#[derive(Debug, Clone)]
pub struct PhaseResult {
    pub tier: ModelTier,
    pub model: String,
    pub messages: Vec<ConversationMessage>,
    pub usage: TokenUsage,
    pub plan: Option<Plan>,
    pub success: bool,
    pub summary: String,
}

/// Accumulated results across all phases.
#[derive(Debug, Clone, Default)]
pub struct OrchestratorResult {
    pub phases: Vec<PhaseResult>,
    pub total_usage: TokenUsage,
    pub final_plan: Option<Plan>,
    pub all_messages: Vec<ConversationMessage>,
}

impl OrchestratorResult {
    #[must_use]
    pub fn total_cost_estimate(&self) -> f64 {
        self.phases
            .iter()
            .map(|p| p.usage.estimate_cost_usd().total_cost_usd())
            .sum()
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let mut lines = vec![format!(
            "Orchestrator complete: {} phases, total estimated cost {}",
            self.phases.len(),
            format_usd(self.total_cost_estimate())
        )];
        for phase in &self.phases {
            lines.push(format!(
                "  {} phase ({}): {} tokens, {}",
                phase.tier,
                phase.model,
                phase.usage.total_tokens(),
                if phase.success { "OK" } else { "FAILED" }
            ));
        }
        lines.join("\n")
    }
}

/// Error from orchestrator operations.
#[derive(Debug, Clone)]
pub struct OrchestratorError {
    pub phase: Option<ModelTier>,
    pub message: String,
}

impl std::fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.phase {
            Some(tier) => write!(f, "{} phase error: {}", tier, self.message),
            None => write!(f, "orchestrator error: {}", self.message),
        }
    }
}

impl std::error::Error for OrchestratorError {}

/// Factory trait for creating API clients for a given model.
///
/// This allows the orchestrator to switch models between phases
/// without coupling to a specific client implementation.
pub trait ClientFactory {
    fn create(&self, model: &str) -> Result<Box<dyn ApiClient>, String>;
}

impl<F> ClientFactory for F
where
    F: Fn(&str) -> Result<Box<dyn ApiClient>, String>,
{
    fn create(&self, model: &str) -> Result<Box<dyn ApiClient>, String> {
        (self)(model)
    }
}

/// The orchestrator manages the plan→execute→verify pipeline,
/// routing each phase to the appropriate model.
pub struct Orchestrator {
    config: RouterConfig,
    client_factory: Box<dyn ClientFactory>,
    system_prompt: Vec<String>,
}

impl Orchestrator {
    #[must_use]
    pub fn new(
        config: RouterConfig,
        client_factory: Box<dyn ClientFactory>,
        system_prompt: Vec<String>,
    ) -> Self {
        Self {
            config,
            client_factory,
            system_prompt,
        }
    }

    /// Run the full pipeline for a given user task.
    ///
    /// Returns accumulated results from all phases.
    pub fn run(&mut self, task: &str) -> Result<OrchestratorResult, OrchestratorError> {
        if !self.config.enabled {
            return Err(OrchestratorError {
                phase: None,
                message: "model-router is disabled".to_string(),
            });
        }

        let mut result = OrchestratorResult::default();

        // Phase 1: Planning (if strategy includes it)
        let plan = if self.config.strategy.has_planning_phase() {
            let phase = self.run_planning_phase(task)?;
            let plan = phase.plan.clone();
            result.total_usage = accumulate_usage(result.total_usage, phase.usage);
            result.phases.push(phase);
            plan
        } else {
            None
        };

        // Phase 2: Execution (if strategy includes it)
        if self.config.strategy.has_execution_phase() {
            let exec_result = self.run_execution_phase(task, &plan)?;
            result.total_usage = accumulate_usage(result.total_usage, exec_result.usage);
            let mut final_plan = plan.clone();
            if let Some(ref mut p) = final_plan {
                for step in &mut p.steps {
                    step.completed = true;
                }
            }
            result.final_plan = final_plan;
            result.all_messages.extend(exec_result.messages.clone());
            result.phases.push(exec_result);
        } else if let Some(ref p) = plan {
            result.final_plan = Some(p.clone());
        }

        // Phase 3: Verification (if strategy includes it)
        if self.config.strategy.has_verification_phase() {
            let phase =
                self.run_verification_phase(task, &result.final_plan, &result.all_messages)?;
            result.total_usage = accumulate_usage(result.total_usage, phase.usage);
            result.phases.push(phase);
        }

        Ok(result)
    }

    /// Run the planning phase: send the task to the planner model, get a structured plan.
    fn run_planning_phase(&mut self, task: &str) -> Result<PhaseResult, OrchestratorError> {
        let model = self.config.model_for_tier(ModelTier::Planner).to_string();
        let mut client = self.make_client(&model, ModelTier::Planner)?;

        let planning_prompt = build_planning_prompt(task);
        let request = build_text_request(&self.system_prompt, &planning_prompt);

        let events = client.stream(request).map_err(|e| OrchestratorError {
            phase: Some(ModelTier::Planner),
            message: e.to_string(),
        })?;

        let (text, usage) = collect_text_and_usage(&events);
        let plan = Plan::parse(&text, task);

        Ok(PhaseResult {
            tier: ModelTier::Planner,
            model,
            messages: vec![
                ConversationMessage::user_text(&planning_prompt),
                assistant_text_message(&text),
            ],
            usage,
            plan: Some(plan),
            success: true,
            summary: format!("Planning complete: {} steps", text.lines().count()),
        })
    }

    /// Run the execution phase: feed the plan to the executor model.
    fn run_execution_phase(
        &mut self,
        task: &str,
        plan: &Option<Plan>,
    ) -> Result<PhaseResult, OrchestratorError> {
        let model = self.config.model_for_tier(ModelTier::Executor).to_string();
        let mut client = self.make_client(&model, ModelTier::Executor)?;

        let exec_prompt = if let Some(ref plan) = plan {
            plan.executor_prompt()
        } else {
            format!("Complete this task using the available tools:\n\n{task}")
        };

        let request = build_text_request(&self.system_prompt, &exec_prompt);

        let events = client.stream(request).map_err(|e| OrchestratorError {
            phase: Some(ModelTier::Executor),
            message: e.to_string(),
        })?;

        let (text, usage) = collect_text_and_usage(&events);

        Ok(PhaseResult {
            tier: ModelTier::Executor,
            model,
            messages: vec![
                ConversationMessage::user_text(&exec_prompt),
                assistant_text_message(&text),
            ],
            usage,
            plan: None,
            success: true,
            summary: format!("Execution complete: {} output tokens", usage.output_tokens),
        })
    }

    /// Run the verification phase: send results to the verifier model.
    fn run_verification_phase(
        &mut self,
        task: &str,
        plan: &Option<Plan>,
        _exec_messages: &[ConversationMessage],
    ) -> Result<PhaseResult, OrchestratorError> {
        let model = self.config.model_for_tier(ModelTier::Verifier).to_string();
        let mut client = self.make_client(&model, ModelTier::Verifier)?;

        let verify_prompt = if let Some(ref plan) = plan {
            plan.verifier_prompt()
        } else {
            format!(
                "Verify the following work was done correctly.\n\n## Task\n{task}\n\n## Work Done\nPlease review and confirm correctness."
            )
        };

        let request = build_text_request(&self.system_prompt, &verify_prompt);

        let events = client.stream(request).map_err(|e| OrchestratorError {
            phase: Some(ModelTier::Verifier),
            message: e.to_string(),
        })?;

        let (text, usage) = collect_text_and_usage(&events);
        let passed = text.to_ascii_uppercase().contains("VERIFICATION PASSED");

        Ok(PhaseResult {
            tier: ModelTier::Verifier,
            model,
            messages: vec![
                ConversationMessage::user_text(&verify_prompt),
                assistant_text_message(&text),
            ],
            usage,
            plan: None,
            success: passed,
            summary: if passed {
                "Verification passed".to_string()
            } else {
                format!(
                    "Verification found issues: {}",
                    text.lines().next().unwrap_or("unknown")
                )
            },
        })
    }

    fn make_client(
        &self,
        model: &str,
        tier: ModelTier,
    ) -> Result<Box<dyn ApiClient>, OrchestratorError> {
        self.client_factory
            .create(model)
            .map_err(|e| OrchestratorError {
                phase: Some(tier),
                message: e,
            })
    }
}

/// Build a system prompt instructing the model to create a detailed plan.
fn build_planning_prompt(task: &str) -> String {
    format!(
        "\
You are a planning specialist. Your job is to create a detailed, step-by-step execution plan.

## Task
{task}

## Instructions
1. Analyze the task and break it down into concrete, actionable steps.
2. Each step should be specific and verifiable.
3. Use a numbered list format: \"1. First step\", \"2. Second step\", etc.
4. Include file paths when relevant.
5. Do NOT execute anything — just plan.

## Plan
"
    )
}

/// Build a simple text-only API request.
fn build_text_request(system_prompt: &[String], user_text: &str) -> ApiRequest {
    ApiRequest {
        system_prompt: system_prompt.to_vec(),
        messages: vec![ConversationMessage::user_text(user_text)],
    }
}

/// Create an assistant text message.
fn assistant_text_message(text: &str) -> ConversationMessage {
    ConversationMessage::assistant(vec![ContentBlock::Text {
        text: text.to_string(),
    }])
}

/// Collect all text content and token usage from a stream of events.
fn collect_text_and_usage(events: &[AssistantEvent]) -> (String, TokenUsage) {
    let mut text = String::new();
    let mut usage = TokenUsage::default();

    for event in events {
        match event {
            AssistantEvent::TextDelta(delta) => text.push_str(delta),
            AssistantEvent::Thinking { thinking, .. } => text.push_str(thinking),
            AssistantEvent::ToolUse { name, input, .. } => {
                text.push_str(&format!("\n[Tool: {} with input: {}]\n", name, input));
            }
            AssistantEvent::Usage(u) => usage = *u,
            _ => {}
        }
    }

    (text, usage)
}

fn accumulate_usage(current: TokenUsage, add: TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: current.input_tokens + add.input_tokens,
        output_tokens: current.output_tokens + add.output_tokens,
        cache_creation_input_tokens: current.cache_creation_input_tokens
            + add.cache_creation_input_tokens,
        cache_read_input_tokens: current.cache_read_input_tokens + add.cache_read_input_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockClient {
        response: String,
        usage: TokenUsage,
    }

    impl ApiClient for MockClient {
        fn stream(
            &mut self,
            _request: ApiRequest,
        ) -> Result<Vec<AssistantEvent>, runtime::RuntimeError> {
            Ok(vec![
                AssistantEvent::TextDelta(self.response.clone()),
                AssistantEvent::Usage(self.usage),
            ])
        }
    }

    fn mock_factory(response: &str) -> Box<dyn ClientFactory> {
        let resp = response.to_string();
        Box::new(move |_model: &str| -> Result<Box<dyn ApiClient>, String> {
            Ok(Box::new(MockClient {
                response: resp.clone(),
                usage: TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                },
            }))
        })
    }

    #[test]
    fn orchestrator_plan_execute_verify_with_mock() {
        let config = RouterConfig {
            planner_model: "test-planner".into(),
            executor_model: "test-executor".into(),
            verifier_model: "test-verifier".into(),
            strategy: RoutingStrategy::PlanExecuteVerify,
            enabled: true,
        };

        let plan_text = "1. Create a config file\n2. Add the endpoint\n3. Write tests";
        let exec_text = "I have completed the implementation.";
        let verify_text = "VERIFICATION PASSED. All steps completed correctly.";

        // Each call advances through the responses
        let responses = vec![
            plan_text.to_string(),
            exec_text.to_string(),
            verify_text.to_string(),
        ];
        let call_idx = std::cell::Cell::new(0usize);
        let factory = Box::new(move |_model: &str| -> Result<Box<dyn ApiClient>, String> {
            let idx = call_idx.get();
            let resp = responses[idx].clone();
            call_idx.set(idx + 1);
            Ok(Box::new(MockClient {
                response: resp,
                usage: TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                },
            }))
        });

        let mut orchestrator =
            Orchestrator::new(config, factory, vec!["Test system prompt".into()]);
        let result = orchestrator
            .run("Add a new API endpoint")
            .expect("orchestrator should complete");

        assert_eq!(result.phases.len(), 3);
        assert_eq!(result.phases[0].tier, ModelTier::Planner);
        assert_eq!(result.phases[1].tier, ModelTier::Executor);
        assert_eq!(result.phases[2].tier, ModelTier::Verifier);
        assert!(result.phases[2].success, "verification should pass");
        assert_eq!(result.final_plan.unwrap().steps.len(), 3);
    }

    #[test]
    fn orchestrator_plan_only_skips_verification() {
        let config = RouterConfig {
            planner_model: "planner".into(),
            executor_model: "executor".into(),
            verifier_model: "verifier".into(),
            strategy: RoutingStrategy::PlanOnly,
            enabled: true,
        };

        let plan_text = "1. Step one\n2. Step two";
        let exec_text = "Done.";

        let responses = vec![plan_text.to_string(), exec_text.to_string()];
        let call_idx = std::cell::Cell::new(0usize);
        let factory = Box::new(move |_model: &str| -> Result<Box<dyn ApiClient>, String> {
            let idx = call_idx.get();
            let resp = responses[idx].clone();
            call_idx.set(idx + 1);
            Ok(Box::new(MockClient {
                response: resp,
                usage: TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                },
            }))
        });

        let mut orchestrator = Orchestrator::new(config, factory, vec![]);
        let result = orchestrator.run("test").expect("should complete");
        assert_eq!(result.phases.len(), 2, "plan-only has 2 phases");
    }

    #[test]
    fn disabled_router_returns_error() {
        let config = RouterConfig {
            enabled: false,
            ..Default::default()
        };
        let factory = mock_factory("irrelevant");
        let mut orchestrator = Orchestrator::new(config, factory, vec![]);
        let err = orchestrator.run("test").expect_err("should be disabled");
        assert!(err.to_string().contains("disabled"));
    }

    #[test]
    fn collect_text_and_usage_extracts_tool_calls() {
        let events = vec![
            AssistantEvent::TextDelta("I will ".to_string()),
            AssistantEvent::ToolUse {
                id: "tool_1".to_string(),
                name: "read".to_string(),
                input: r#"{"file":"test.rs"}"#.to_string(),
            },
            AssistantEvent::TextDelta("read the file".to_string()),
            AssistantEvent::Usage(TokenUsage {
                input_tokens: 200,
                output_tokens: 100,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            }),
        ];
        let (text, usage) = collect_text_and_usage(&events);
        assert!(text.contains("[Tool: read with input:"));
        assert!(text.contains("I will "));
        assert_eq!(usage.output_tokens, 100);
    }
}
