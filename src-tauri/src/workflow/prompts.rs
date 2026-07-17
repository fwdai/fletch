//! Step protocol prompt assembly (spec §8.5). Pure string building: given the
//! run task, the step goal, position, gate, turn budget, and declared comms
//! caps, produce the prompt the scheduler sends to a step agent. A step with
//! `report`/`ask` caps gets the comms section (§8.5 item 6) appended, describing
//! how to call the `wf_*` RPC ops through the mailbox.
//!
//! Everything here is deterministic and side-effect free so the exact text is
//! unit-testable and stable across runs.

use super::spec::{CommsCap, Gate, Require};

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
    /// Launch-time file attachments (absolute paths), rendered as
    /// `Attached file: {path}` lines. Non-empty only for the run's entry step —
    /// they belong to the initial task, like a chat message's attachments, not
    /// to every step — so most prompts pass an empty slice.
    pub attachments: &'a [String],
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

/// Everything the orchestrator prompt is assembled from (spec §10.2, §6.6). The
/// orchestrator is a single agent that lives for the whole stage: it is prompted
/// once here, again whenever a child asks it something or finishes, and a final
/// time for its concluding verdict.
#[derive(Debug, Clone)]
pub struct OrchestratorPromptCtx<'a> {
    pub run_task: &'a str,
    /// The orchestrator's blackboard directory id (`orchestrate-<n>`).
    pub orch_step_id: &'a str,
    pub goal: &'a str,
    pub position: Position,
    /// Ids of static children that auto-start at stage entry (may be empty).
    pub static_children: &'a [String],
    /// The dynamic-child template: `(agent alias, max)` when the block declares
    /// `children`; `None` when only static children exist.
    pub dynamic: Option<(&'a str, u32)>,
    /// `max_sub_runs` when the block enables dynamic composition (spec §10.3);
    /// `None` leaves `wf_compose` out of the prompt entirely.
    pub compose_max_sub_runs: Option<u32>,
}

/// The orchestrator's opening prompt (spec §10.2). Describes its supervisory
/// role, the children it commands, and the `wf_decide` / `wf_notify` ops it drives
/// them with — mirroring the step protocol's mailbox style (§8.5 item 6).
pub fn orchestrator_prompt(ctx: &OrchestratorPromptCtx) -> String {
    let mut s = String::new();
    s.push_str("# Workflow orchestrator\n\n");
    s.push_str(
        "You lead a stage of an automated workflow. Child agents do the work; \
         you assign it, answer their questions, and decide when the stage is \
         done. The deterministic workflow engine validates and executes every \
         decision you make — you advise, it acts.\n\n",
    );

    s.push_str("## The overall task\n\n");
    s.push_str(ctx.run_task.trim());
    s.push_str("\n\n");

    s.push_str("## Your goal\n\n");
    s.push_str(&position_line(&ctx.position));
    s.push_str("\n\n");
    s.push_str(ctx.goal.trim());
    s.push_str("\n\n");

    s.push_str("## Your children\n\n");
    if ctx.static_children.is_empty() {
        s.push_str("No children have started yet.");
    } else {
        s.push_str("These children start automatically now:\n");
        for id in ctx.static_children {
            s.push_str(&format!("- `{id}`\n"));
        }
    }
    if let Some((agent, max)) = ctx.dynamic {
        s.push_str(&format!(
            "\nYou may spawn up to **{max}** additional `{agent}` children with \
             `wf_decide` (`spawn_child`). Nothing spawns from this template on its \
             own — you decide how many, if any.\n",
        ));
    }
    s.push('\n');

    s.push_str(&orchestrator_comms_section(ctx.compose_max_sub_runs));
    s.push('\n');

    s.push_str(&blackboard_contract(ctx.orch_step_id));
    s.push('\n');

    s.push_str("## Ending the stage\n\n");
    s.push_str(
        "The stage ends when every child has finished **and** you have concluded. \
         When the children are done, the workflow prompts you once more to write \
         your `verdict.json` (`result` \"done\"). If the plan is already satisfied \
         and remaining children are unnecessary, you may end early with \
         `wf_decide` (`stage_done`).\n",
    );
    s
}

/// The concluding-verdict prompt (spec §6.6): sent once the join condition over
/// all children is met, asking the orchestrator to write its verdict so the
/// stage's gate (its own verdict) can be evaluated.
pub fn orchestrator_conclude_prompt(orch_step_id: &str) -> String {
    let mut s = String::new();
    s.push_str("## All children are done\n\n");
    s.push_str(
        "Every child in this stage has reached a terminal state. Review their \
         handoffs and verdicts on the blackboard, then conclude the stage: write \
         your summary to your `handoff.md` and your completion signal to \
         `verdict.json` with `result` \"done\". The stage does not advance until \
         you do.\n\n",
    );
    s.push_str(&format!(
        "Write `{orch_step_id}/verdict.json`:\n\n{}",
        verdict_schema_block()
    ));
    s
}

/// The orchestrator's comms section: it holds every cap (spec §5.1 — "orchestrator
/// gets all"), so it may answer/route with `wf_decide`, push notices to children
/// with `wf_notify`, and escalate to the human. When the stage enables dynamic
/// composition (spec §10.3), `wf_compose` is described too — an op the agent can
/// only discover here. Mirrors the git-actions mailbox style used by the step
/// protocol (§8.5 item 6).
fn orchestrator_comms_section(compose_max_sub_runs: Option<u32>) -> String {
    let mut s = String::new();
    s.push_str("## Directing the workflow\n\n");
    s.push_str(
        "Send structured messages to the workflow host through your RPC mailbox \
         (the same `$FLETCH_RPC_DIR` request/response channel the git actions \
         use): write `requests/<id>.json` with an `op` and `args`, then read \
         `responses/<id>.json`. Child questions and completion notices are \
         delivered to you at the start of your turns.\n\n",
    );
    s.push_str(
        "- **Decide** — `op: \"wf_decide\"`. One decision per call:\n\
         \x20 - `{ \"decision\": \"answer\", \"message_id\": \"…\", \"body\": \"…\" }` — \
         reply to a child's question (use the `message_id` from the delivered ask).\n\
         \x20 - `{ \"decision\": \"spawn_child\", \"agent\": \"<alias>\", \"goal\": \"…\" }` — \
         start another child (within your template's max).\n\
         \x20 - `{ \"decision\": \"skip_child\", \"step_id\": \"…\", \"reason\": \"…\" }` — \
         drop a child you no longer need.\n\
         \x20 - `{ \"decision\": \"retry_child\", \"step_id\": \"…\", \"guidance\": \"…\" }` — \
         ask a finished child to try again with guidance.\n\
         \x20 - `{ \"decision\": \"stage_done\" }` — end the stage now (the plan is \
         satisfied).\n\
         \x20 - `{ \"decision\": \"escalate\", \"question\": \"…\" }` — hand a decision to \
         the human; the run pauses until they answer.\n",
    );
    s.push_str(
        "- **Notify** — `op: \"wf_notify\"`, \
         `args: { \"to\": \"<step-id>\" | \"all-children\", \"message\": \"…\" }`. \
         Push a notice to a running child (e.g. a sibling's slice landed).\n",
    );
    if let Some(max) = compose_max_sub_runs {
        s.push_str(&format!(
            "- **Compose a sub-workflow** — `op: \"wf_compose\"`, \
             `args: {{ \"task\": \"…\", \"fragment\": [ {{ \"step\": {{ \"id\": \"…\", \
             \"agent\": \"<alias>\", \"goal\": \"…\" }} }}, … ], \
             \"budgets\": {{ \"turns\": <n> }}, \"integrate\": \"none\" | \"merge\", \
             \"base\": \"parent-head\" | \"run-base\" }}`. \
             When the work needs a multi-step pipeline rather than one more child, \
             describe it as a fragment of workflow blocks and the engine runs it as \
             a sub-run that joins this stage. `budgets.turns` reserves a slice of \
             this run's remaining turn budget (required; `budgets.tokens` optional). \
             `agents` is an optional agent map — omit it to reuse this run's agents. \
             The engine validates the fragment and enforces the limits; you may \
             launch at most **{max}** sub-run{s_} this stage.\n",
            s_ = if max == 1 { "" } else { "s" },
        ));
    }
    s
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
        Gate::Approval { require } if require.contains(&Require::Tests) => {
            "When you believe the work is complete, write your handoff notes and \
             `verdict.json`. The project's test suite must pass first — the workflow \
             runs it after your turn; make it green. A human then reviews and approves \
             before the workflow continues."
                .to_string()
        }
        Gate::Approval { .. } => "When you believe the work is complete, write your handoff notes \
             and `verdict.json`. A human will review and approve before the workflow \
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
            attachments: &[],
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
        assert!(gate_statement(&Gate::Approval { require: vec![] }).contains("human"));
        let tests = gate_statement(&Gate::Tests);
        assert!(tests.contains("test suite passes"), "{tests}");
        // An approval gate that requires tests reminds the agent about them too.
        let approval_tests = gate_statement(&Gate::Approval {
            require: vec![Require::Tests],
        });
        assert!(approval_tests.contains("test suite"), "{approval_tests}");
        assert!(approval_tests.contains("human"), "{approval_tests}");
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
    fn orchestrator_prompt_lists_children_and_decisions() {
        let statics = vec!["review".to_string()];
        let p = orchestrator_prompt(&OrchestratorPromptCtx {
            run_task: "Ship the feature",
            orch_step_id: "orchestrate-1",
            goal: "Assign slices and answer questions",
            position: Position {
                step_index: 1,
                step_count: 3,
                iteration: None,
            },
            static_children: &statics,
            dynamic: Some(("coder", 3)),
            compose_max_sub_runs: None,
        });
        assert!(p.contains("Ship the feature"));
        assert!(p.contains("Assign slices"));
        assert!(p.contains("`review`"), "static child listed: {p}");
        assert!(
            p.contains("up to **3**") && p.contains("`coder`"),
            "dynamic template: {p}"
        );
        // The decision surface is described.
        assert!(p.contains("wf_decide"));
        assert!(p.contains("spawn_child") && p.contains("stage_done") && p.contains("escalate"));
        assert!(p.contains("wf_notify"));
        // Composition is disabled for this stage, so the op is never mentioned.
        assert!(!p.contains("wf_compose"), "compose off ⇒ unadvertised: {p}");
        // Writes its own verdict to conclude.
        assert!(p.contains("orchestrate-1/verdict.json"));
    }

    #[test]
    fn orchestrator_prompt_advertises_compose_only_when_enabled() {
        let p = orchestrator_prompt(&OrchestratorPromptCtx {
            run_task: "Ship the feature",
            orch_step_id: "orchestrate-0",
            goal: "Split and supervise",
            position: Position {
                step_index: 0,
                step_count: 1,
                iteration: None,
            },
            static_children: &[],
            dynamic: None,
            compose_max_sub_runs: Some(2),
        });
        assert!(p.contains("wf_compose"), "{p}");
        // The contract's load-bearing pieces: fragment shape, required budget
        // slice, integrate/base vocabulary, and the stage limit.
        assert!(p.contains("\"fragment\""));
        assert!(p.contains("\"budgets\""));
        assert!(p.contains("\"turns\""));
        assert!(p.contains("\"parent-head\"") && p.contains("\"run-base\""));
        assert!(p.contains("at most **2** sub-runs"), "{p}");
    }

    #[test]
    fn conclude_prompt_asks_for_the_verdict() {
        let p = orchestrator_conclude_prompt("orchestrate-0");
        assert!(p.contains("All children are done"));
        assert!(p.contains("orchestrate-0/verdict.json"));
        assert!(p.contains("\"result\""));
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
