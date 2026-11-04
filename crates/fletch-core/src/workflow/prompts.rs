//! Step protocol prompt assembly (spec §8.5). Pure string building: given the
//! run task, the step goal, its position in the run, and its gate, produce the
//! prompt the kernel sends to a step agent.
//!
//! Everything here is deterministic and side-effect free so the exact text is
//! unit-testable and stable across runs.

use super::spec::Gate;

/// Everything the step prompt is assembled from (spec §8.5 items 1–5).
#[derive(Debug, Clone)]
pub struct StepPromptCtx<'a> {
    /// The run's overall task (§8.5 item 1).
    pub run_task: &'a str,
    /// Launch-time file attachments (absolute paths), rendered as
    /// `Attached file: {path}` lines. Non-empty only for the run's entry step —
    /// they belong to the initial task, like a chat message's attachments, not
    /// to every step — so most prompts pass an empty slice.
    pub attachments: &'a [String],
    /// The step's id — names its blackboard directory.
    pub step_id: &'a str,
    /// The step goal (§8.5 item 2).
    pub step_goal: &'a str,
    /// 0-based index of the step in the run, for the "step N of M" line (§8.5
    /// item 3).
    pub step_index: usize,
    /// How many steps the run has.
    pub step_count: usize,
    /// The gate that decides this step is done (§8.5 item 5).
    pub gate: &'a Gate,
}

/// The full step prompt (spec §8.5).
pub fn step_prompt(ctx: &StepPromptCtx) -> String {
    let mut s = String::new();

    s.push_str("# Workflow step\n\n");
    s.push_str("You are one step in an automated workflow. Complete the goal below, ");
    s.push_str("leave handoff notes for the steps that follow, and signal completion ");
    s.push_str("exactly as the gate requires.\n\n");

    s.push_str("## The overall task\n\n");
    s.push_str(ctx.run_task.trim());
    s.push_str("\n\n");

    // Launch attachments (entry step only): the same `Attached file: {path}`
    // reference lines a chat message delivers, so the first agent can read them.
    if !ctx.attachments.is_empty() {
        s.push_str("## Attached files\n\n");
        s.push_str("Files attached to the task — read them as needed:\n\n");
        for path in ctx.attachments {
            s.push_str("Attached file: ");
            s.push_str(path);
            s.push('\n');
        }
        s.push('\n');
    }

    s.push_str("## Your step\n\n");
    s.push_str(&format!(
        "This is step {} of {}.",
        ctx.step_index + 1,
        ctx.step_count
    ));
    s.push_str("\n\n");
    s.push_str(ctx.step_goal.trim());
    s.push_str("\n\n");

    s.push_str(&blackboard_contract(ctx.step_id));
    s.push('\n');

    s.push_str("## Done when\n\n");
    s.push_str(&gate_statement(ctx.gate));
    s.push_str("\n\n");

    s
}

fn blackboard_contract(step_id: &str) -> String {
    format!(
        "## Blackboard\n\n\
         A shared blackboard directory is available at the path in the \
         `WF_BLACKBOARD` environment variable. Use it to hand work off:\n\n\
         - **Read** prior steps' notes under `<step-id>/handoff.md` and any \
         `verdict.json` they left, plus anything in `shared/`.\n\
         - **Write** your own handoff into `{step_id}/handoff.md` (free-form \
         notes for the steps after you).\n\
         - **Write** your completion signal into `{step_id}/verdict.json`.\n\
         - You may use `shared/` for cross-step scratch space. Only write inside \
         your own `{step_id}/` directory and `shared/`.\n\n\
         {schema}",
        step_id = step_id,
        schema = verdict_schema_block(),
    )
}

fn verdict_schema_block() -> String {
    "`verdict.json` schema:\n\n\
     ```json\n\
     {\n\
     \x20 \"result\": \"done\" | \"revise\" | \"blocked\",\n\
     \x20 \"summary\": \"one-line handoff for the timeline and the next step\",\n\
     \x20 \"detail\": \"optional; e.g. structured feedback\",\n\
     \x20 \"target\": \"optional step-id (revise only)\"\n\
     }\n\
     ```\n"
        .to_string()
}

fn gate_statement(gate: &Gate) -> String {
    match gate {
        Gate::Verdict => {
            "You are done when you write `verdict.json` with `result` set to \"done\".".to_string()
        }
        Gate::Commit => {
            "You are done when you have made at least one git commit in this repository."
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(gate: &'a Gate, run_task: &'a str, goal: &'a str) -> StepPromptCtx<'a> {
        StepPromptCtx {
            run_task,
            attachments: &[],
            step_id: "plan",
            step_goal: goal,
            step_index: 0,
            step_count: 3,
            gate,
        }
    }

    #[test]
    fn step_prompt_includes_task_goal_position_and_schema() {
        let gate = Gate::Verdict;
        let p = step_prompt(&ctx(&gate, "Ship the feature", "Write PLAN.md"));
        assert!(p.contains("Ship the feature"));
        assert!(p.contains("Write PLAN.md"));
        assert!(p.contains("step 1 of 3"));
        assert!(p.contains("WF_BLACKBOARD"));
        assert!(p.contains("plan/verdict.json"));
        assert!(p.contains("\"result\""));
    }

    #[test]
    fn attachments_render_only_when_present() {
        let gate = Gate::Verdict;
        // Default (entry has none / non-entry step) → no attachment section.
        assert!(!step_prompt(&ctx(&gate, "task", "goal")).contains("Attached file:"));
        // Entry step with launch attachments → each renders as a reference line.
        let atts = vec!["/tmp/a.txt".to_string(), "/tmp/b.png".to_string()];
        let mut c = ctx(&gate, "task", "goal");
        c.attachments = &atts;
        let p = step_prompt(&c);
        assert!(p.contains("## Attached files"), "{p}");
        assert!(p.contains("Attached file: /tmp/a.txt"));
        assert!(p.contains("Attached file: /tmp/b.png"));
    }

    #[test]
    fn gate_statement_matches_gate() {
        assert!(gate_statement(&Gate::Verdict).contains("verdict.json"));
        assert!(gate_statement(&Gate::Commit).contains("commit"));
    }
}
