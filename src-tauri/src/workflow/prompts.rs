//! Step protocol prompt assembly (spec §8.5). Pure string building: given the
//! run task, the step goal, position, gate, turn budget, and declared comms
//! caps, produce the prompt the scheduler sends to a step agent. A step with
//! `report`/`ask` caps gets the comms section (§8.5 item 6) appended, describing
//! how to call the `wf_*` RPC ops through the mailbox.
//!
//! Everything here is deterministic and side-effect free so the exact text is
//! unit-testable and stable across runs.

use super::spec::{CommsCap, Gate};

/// Where a step sits in the run, for the "step N of M, iteration i of max"
/// context line (spec §8.5 item 3).
#[derive(Debug, Clone)]
pub struct Position {
    /// 0-based index within its enclosing sequence.
    pub step_index: usize,
    /// Total steps in the enclosing sequence.
    pub step_count: usize,
    /// Loop iteration, when the step is inside a loop body.
    pub iteration: Option<IterationPos>,
}

/// 1-based loop iteration and the loop's declared max.
#[derive(Debug, Clone)]
pub struct IterationPos {
    pub current: u32,
    pub max: u32,
}

/// Everything the step prompt is assembled from (spec §8.5 items 1–5, 7).
#[derive(Debug, Clone)]
pub struct StepPromptCtx<'a> {
    /// The run's overall task (§8.5 item 1).
    pub run_task: &'a str,
    /// The step's id — names its blackboard directory.
    pub step_id: &'a str,
    /// The step goal (§8.5 item 2).
    pub step_goal: &'a str,
    /// Position context (§8.5 item 3).
    pub position: Position,
    /// The gate that decides this step is done (§8.5 item 5).
    pub gate: &'a Gate,
    /// The per-attempt turn budget, when one applies (§8.5 item 7).
    pub turns_per_attempt: Option<i64>,
    /// The step's declared comms caps (§8.5 item 6). The comms section is
    /// appended only when this is non-empty.
    pub comms: &'a [CommsCap],
}

/// The full step prompt for a fresh attempt (spec §8.5).
pub fn step_prompt(ctx: &StepPromptCtx) -> String {
    let mut s = String::new();

    s.push_str("# Workflow step\n\n");
    s.push_str("You are one step in an automated workflow. Complete the goal below, ");
    s.push_str("leave handoff notes for the steps that follow, and signal completion ");
    s.push_str("exactly as the gate requires.\n\n");

    s.push_str("## The overall task\n\n");
    s.push_str(ctx.run_task.trim());
    s.push_str("\n\n");

    s.push_str("## Your step\n\n");
    s.push_str(&position_line(&ctx.position));
    s.push_str("\n\n");
    s.push_str(ctx.step_goal.trim());
    s.push_str("\n\n");

    s.push_str(&blackboard_contract(ctx.step_id));
    s.push('\n');

    s.push_str("## Done when\n\n");
    s.push_str(&gate_statement(ctx.gate));
    s.push_str("\n\n");

    if let Some(comms) = comms_section(ctx.comms) {
        s.push_str(&comms);
        s.push('\n');
    }

    if let Some(budget) = budget_notice(ctx.turns_per_attempt) {
        s.push_str(&budget);
        s.push('\n');
    }

    s
}

/// The comms section (spec §8.5 item 6): how to talk to the host through the RPC
/// mailbox, mirroring the git-actions playbook style. Only the ops the step is
/// permitted to call are described. `None` when the step has no caps.
fn comms_section(caps: &[CommsCap]) -> Option<String> {
    let can_report = caps.contains(&CommsCap::Report);
    let can_ask = caps.contains(&CommsCap::Ask);
    if !can_report && !can_ask {
        return None;
    }

    let mut s = String::new();
    s.push_str("## Talking to the workflow\n\n");
    s.push_str(
        "You can send structured messages to the workflow host through your RPC \
         mailbox (the same `$FLETCH_RPC_DIR` request/response channel the git \
         actions use): write `requests/<id>.json` with an `op` and `args`, then \
         read `responses/<id>.json`.\n\n",
    );
    if can_report {
        s.push_str(
            "- **Report progress** — `op: \"wf_report\"`, \
             `args: { \"status\": \"progress\" | \"done\", \"note\": \"…\" }`. \
             Use it to surface a milestone or your final status; it never ends \
             your turn or replaces `verdict.json`.\n",
        );
    }
    if can_ask {
        s.push_str(
            "- **Ask a question** — `op: \"wf_ask\"`, \
             `args: { \"question\": \"…\", \"options\": [\"…\"] }` (options \
             optional). Call it when you are genuinely blocked on a decision only \
             a human can make, **then end your turn**: the workflow pauses, and \
             you are re-prompted with the answer on your next turn. Don't ask \
             about anything you can decide yourself.\n",
        );
    }
    Some(s)
}

/// The re-prompt sent when the gate is unmet but the attempt still has turns
/// left (spec §6.5) — quotes the gate reason and asks the agent to finish.
pub fn reprompt(gate: &Gate, gate_reason: &str) -> String {
    let mut s = String::new();
    s.push_str("Your step is not done yet: ");
    s.push_str(gate_reason.trim());
    s.push_str(".\n\n");
    s.push_str("Finish the work and satisfy the gate. As a reminder:\n\n");
    s.push_str(&gate_statement(gate));
    s.push('\n');
    s.push_str(&verdict_schema_block());
    s
}

/// The single stall nudge (spec §11.3): the agent has gone quiet mid-turn.
/// The comms half ("reply via wf_report if blocked") lands with S10.
pub fn nudge() -> String {
    "You've gone quiet. Please finish up, write your handoff notes and \
     `verdict.json`, and end your turn. If you are stuck, say so explicitly \
     in `verdict.json` with result \"blocked\" and a summary of what blocked you."
        .to_string()
}

/// The prompt for a fresh attempt after a previous one errored (spec §6.5):
/// the previous failure, summarized, followed by the full step prompt.
pub fn retry_prompt(previous_failure: &str, ctx: &StepPromptCtx) -> String {
    let mut s = String::new();
    s.push_str("## Previous attempt failed\n\n");
    s.push_str("A prior attempt at this step did not complete: ");
    s.push_str(previous_failure.trim());
    s.push_str("\n\nStart fresh from the current repository state and try again.\n\n");
    s.push_str(&step_prompt(ctx));
    s
}

fn position_line(pos: &Position) -> String {
    let mut line = format!("This is step {} of {}.", pos.step_index + 1, pos.step_count);
    if let Some(it) = &pos.iteration {
        line.push_str(&format!(
            " You are on iteration {} of at most {}.",
            it.current, it.max
        ));
    }
    line
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
        Gate::Artifact { path } => {
            format!("You are done when the file `{path}` exists in the repository.")
        }
        Gate::Approval => "When you believe the work is complete, write your handoff notes and \
             `verdict.json`. A human will review and approve before the workflow \
             continues."
            .to_string(),
        Gate::Tests => "You are done when the project's test suite passes. The workflow runs \
             the project's tests for you after your turn; make them green. Still write \
             `verdict.json` and handoff notes so the next step has your summary."
            .to_string(),
    }
}

fn budget_notice(turns_per_attempt: Option<i64>) -> Option<String> {
    match turns_per_attempt {
        Some(n) if n > 0 => Some(format!(
            "## Budget\n\nYou have at most {n} turn{} for this step. Work \
             efficiently and don't wait on anything you can decide yourself.",
            if n == 1 { "" } else { "s" }
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(gate: &'a Gate, run_task: &'a str, goal: &'a str) -> StepPromptCtx<'a> {
        StepPromptCtx {
            run_task,
            step_id: "plan",
            step_goal: goal,
            position: Position {
                step_index: 0,
                step_count: 3,
                iteration: None,
            },
            gate,
            turns_per_attempt: Some(3),
            comms: &[],
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
        assert!(p.contains("at most 3 turns"));
    }

    #[test]
    fn iteration_context_renders_when_present() {
        let gate = Gate::Verdict;
        let mut c = ctx(&gate, "task", "goal");
        c.position.iteration = Some(IterationPos { current: 2, max: 3 });
        let p = step_prompt(&c);
        assert!(p.contains("iteration 2 of at most 3"), "{p}");
    }

    #[test]
    fn gate_statement_matches_gate() {
        assert!(gate_statement(&Gate::Verdict).contains("verdict.json"));
        assert!(gate_statement(&Gate::Commit).contains("commit"));
        assert!(gate_statement(&Gate::Artifact {
            path: "X.md".into()
        })
        .contains("X.md"));
        assert!(gate_statement(&Gate::Approval).contains("human"));
        let tests = gate_statement(&Gate::Tests);
        assert!(tests.contains("test suite passes"), "{tests}");
    }

    #[test]
    fn no_comms_section_without_caps() {
        let gate = Gate::Verdict;
        let p = step_prompt(&ctx(&gate, "task", "goal"));
        // A step with no declared caps gets no comms instructions.
        assert!(!p.contains("wf_report"));
        assert!(!p.contains("wf_ask"));
        assert!(!p.contains("Talking to the workflow"));
    }

    #[test]
    fn comms_section_lists_only_declared_caps() {
        let gate = Gate::Verdict;
        // Ask only.
        let mut c = ctx(&gate, "task", "goal");
        c.comms = &[CommsCap::Ask];
        let p = step_prompt(&c);
        assert!(p.contains("Talking to the workflow"));
        assert!(p.contains("wf_ask"));
        assert!(!p.contains("wf_report"), "report not declared: {p}");

        // Report only.
        let mut c = ctx(&gate, "task", "goal");
        c.comms = &[CommsCap::Report];
        let p = step_prompt(&c);
        assert!(p.contains("wf_report"));
        assert!(!p.contains("wf_ask"), "ask not declared: {p}");

        // Both.
        let mut c = ctx(&gate, "task", "goal");
        c.comms = &[CommsCap::Report, CommsCap::Ask];
        let p = step_prompt(&c);
        assert!(p.contains("wf_report") && p.contains("wf_ask"));
    }

    #[test]
    fn reprompt_quotes_the_gate_reason() {
        let r = reprompt(
            &Gate::Verdict,
            "verdict.json result is \"revise\": fix the loop",
        );
        assert!(r.contains("fix the loop"));
        assert!(r.contains("verdict.json"));
    }

    #[test]
    fn retry_prompt_leads_with_the_failure_then_the_step() {
        let gate = Gate::Verdict;
        let c = ctx(&gate, "the task", "the goal");
        let r = retry_prompt("agent errored: spawn_timeout", &c);
        let failure_at = r.find("spawn_timeout").expect("failure present");
        let goal_at = r.find("the goal").expect("goal present");
        assert!(
            failure_at < goal_at,
            "failure should precede the restated step"
        );
    }

    #[test]
    fn budget_notice_omitted_when_absent_or_nonpositive() {
        assert!(budget_notice(None).is_none());
        assert!(budget_notice(Some(0)).is_none());
        assert!(budget_notice(Some(-1)).is_none());
        assert!(budget_notice(Some(5)).unwrap().contains("5 turns"));
        assert!(budget_notice(Some(1)).unwrap().contains("1 turn "));
    }
}
