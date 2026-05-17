use serde::{Deserialize, Serialize};

/// A structured execution plan produced by the planner model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// High-level goal summary.
    pub goal: String,

    /// Ordered list of execution steps.
    pub steps: Vec<PlanStep>,

    /// Raw plan text from the model (preserved for executor context).
    pub raw_text: String,
}

/// A single step in the execution plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step number (1-based).
    pub index: usize,

    /// Human-readable description of what to do.
    pub description: String,

    /// Optional file paths this step is expected to touch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,

    /// Whether this step has been completed.
    #[serde(default)]
    pub completed: bool,
}

impl Plan {
    /// Parse a plan from the planner model's text output.
    ///
    /// Looks for markdown-style numbered lists and extracts steps.
    #[must_use]
    pub fn parse(text: &str, goal: &str) -> Self {
        let steps = extract_steps(text);
        Self {
            goal: goal.to_string(),
            steps,
            raw_text: text.to_string(),
        }
    }

    /// Number of incomplete steps remaining.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.steps.iter().filter(|s| !s.completed).count()
    }

    /// Mark a step as completed by index (1-based).
    pub fn complete_step(&mut self, index: usize) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.index == index) {
            step.completed = true;
        }
    }

    /// Get the next incomplete step, if any.
    #[must_use]
    pub fn next_step(&self) -> Option<&PlanStep> {
        self.steps.iter().find(|s| !s.completed)
    }

    /// Format the plan as a prompt for the executor model.
    #[must_use]
    pub fn executor_prompt(&self) -> String {
        let mut prompt = String::new();
        prompt.push_str("You are executing a pre-defined plan. Follow it precisely.\n\n");
        prompt.push_str("## Overall Goal\n");
        prompt.push_str(&self.goal);
        prompt.push_str("\n\n## Execution Plan\n");

        for step in &self.steps {
            let status = if step.completed { " [DONE]" } else { " [TODO]" };
            prompt.push_str(&format!("{}. {}{}\n", step.index, step.description, status));
        }

        prompt.push_str("\n## Instructions\n");
        prompt.push_str(
            "Work on the next incomplete step. Use the available tools to implement it.\n",
        );
        prompt.push_str("Report what you did after completing the step.\n");

        prompt
    }

    /// Format the plan as a prompt for the verifier model.
    #[must_use]
    pub fn verifier_prompt(&self) -> String {
        let mut prompt = String::new();
        prompt.push_str("You are verifying the result of an execution plan.\n\n");
        prompt.push_str("## Original Goal\n");
        prompt.push_str(&self.goal);
        prompt.push_str("\n\n## Plan That Was Executed\n");

        for step in &self.steps {
            let status = if step.completed {
                "[COMPLETED]"
            } else {
                "[SKIPPED]"
            };
            prompt.push_str(&format!(
                "{}. {} {}\n",
                step.index, step.description, status
            ));
        }

        prompt.push_str("\n## Verification Task\n");
        prompt
            .push_str("Review the work done. Check for correctness, completeness, and quality.\n");
        prompt.push_str(
            "List any issues found. If everything is correct, say 'VERIFICATION PASSED'.\n",
        );

        prompt
    }
}

/// Extract numbered steps from markdown-style plan text.
fn extract_steps(text: &str) -> Vec<PlanStep> {
    let mut steps: Vec<PlanStep> = Vec::new();
    let mut files_for_next_step: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();

        // Match lines like "1. Do something" or "1) Do something" or "- [ ] 1. Do something"
        if let Some(step) = try_parse_step_line(trimmed) {
            // Attach any accumulated file references to the previous step
            if let Some(last) = steps.last_mut() {
                if last.files.is_empty() && !files_for_next_step.is_empty() {
                    last.files = std::mem::take(&mut files_for_next_step);
                }
            }
            steps.push(step);
        } else if trimmed.starts_with("- File:")
            || trimmed.starts_with("- `") && trimmed.contains('.')
        {
            // Capture file path references
            let file = trimmed
                .trim_start_matches("- File:")
                .trim_start_matches("- ")
                .trim()
                .trim_matches('`')
                .to_string();
            if !file.is_empty() {
                files_for_next_step.push(file);
            }
        }
    }

    // Attach any trailing file refs
    if let Some(last) = steps.last_mut() {
        if last.files.is_empty() && !files_for_next_step.is_empty() {
            last.files = std::mem::take(&mut files_for_next_step);
        }
    }

    steps
}

fn try_parse_step_line(line: &str) -> Option<PlanStep> {
    // Match patterns: "1. text", "1) text", "Step 1: text", "#1 text"
    let content = line
        .strip_prefix("- [ ] ")
        .or_else(|| line.strip_prefix("- [x] "))
        .or_else(|| line.strip_prefix("- [X] "))
        .unwrap_or(line);

    // Try "1. text" or "1) text"
    let after_number = content
        .find(". ")
        .and_then(|pos| {
            let prefix = &content[..pos];
            prefix.parse::<usize>().ok()?;
            Some(&content[pos + 2..])
        })
        .or_else(|| {
            content.find(") ").and_then(|pos| {
                let prefix = &content[..pos];
                prefix.parse::<usize>().ok()?;
                Some(&content[pos + 2..])
            })
        });

    if let Some(desc) = after_number {
        let index = content
            .find(|c: char| !c.is_ascii_digit())
            .map_or(1, |pos| content[..pos].parse().unwrap_or(1));
        return Some(PlanStep {
            index,
            description: desc.trim().to_string(),
            files: Vec::new(),
            completed: false,
        });
    }

    // Try "Step N: text"
    if let Some(rest) = content.strip_prefix("Step ") {
        if let Some(colon_pos) = rest.find(':') {
            if let Ok(index) = rest[..colon_pos].trim().parse::<usize>() {
                return Some(PlanStep {
                    index,
                    description: rest[colon_pos + 1..].trim().to_string(),
                    files: Vec::new(),
                    completed: false,
                });
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numbered_list() {
        let text = "1. Create a new file\n2. Edit the file\n3. Test the changes";
        let plan = Plan::parse(text, "test goal");
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].description, "Create a new file");
        assert_eq!(plan.steps[1].index, 2);
    }

    #[test]
    fn parse_with_file_references() {
        let text = "1. Edit the config\n- `src/config.rs`\n2. Update tests\n- `tests/test.rs`";
        let plan = Plan::parse(text, "test");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].files, vec!["src/config.rs"]);
        assert_eq!(plan.steps[1].files, vec!["tests/test.rs"]);
    }

    #[test]
    fn step_lifecycle() {
        let mut plan = Plan::parse("1. A\n2. B\n3. C", "test");
        assert_eq!(plan.remaining(), 3);

        let next = plan.next_step().expect("should have next step");
        assert_eq!(next.index, 1);

        plan.complete_step(1);
        assert_eq!(plan.remaining(), 2);

        let next = plan.next_step().expect("should have next step");
        assert_eq!(next.index, 2);
    }

    #[test]
    fn executor_prompt_includes_plan() {
        let plan = Plan::parse("1. Do A\n2. Do B", "test goal");
        let prompt = plan.executor_prompt();
        assert!(prompt.contains("test goal"));
        assert!(prompt.contains("1. Do A"));
        assert!(prompt.contains("[TODO]"));
    }

    #[test]
    fn verifier_prompt_includes_verification_instructions() {
        let plan = Plan::parse("1. Do A", "test");
        let prompt = plan.verifier_prompt();
        assert!(prompt.contains("VERIFICATION PASSED"));
        assert!(prompt.contains("Do A"));
    }
}
