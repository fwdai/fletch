//! Gate evaluation — the deterministic predicate that decides a step attempt
//! is done (spec §9). Every gate is a **pure** function of already-gathered
//! facts: the caller (`workflow::attempt`) reads git HEADs, the blackboard
//! verdict, and artifact existence, then asks this module for the verdict so
//! the decision is trivially unit-testable and journalable.
//!
//! S4 implemented four gates — `verdict`, `commit`, `artifact`, `approval`.
//! S6 adds the `tests` gate (spec §9.4): the caller (`workflow::tests_gate`)
//! resolves and runs the project's test command bounded in the step worktree,
//! then hands the [`TestsOutcome`] in as a fact — execution stays out of this
//! pure module. When no test command resolves the gate has verified nothing, so
//! it blocks with a named cause rather than falling back to the agent's verdict
//! (that would make `gate: tests` a silent `gate: verdict`, §9.4).

use super::blackboard::{Verdict, VerdictResult};
use super::spec::{Gate, Require};

/// The three terminal shapes a gate evaluation can take. Maps onto the step
/// attempt's `gating → { done | blocked | awaiting_approval }` transition
/// (spec §6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// The gate is satisfied; the attempt completes and the run advances.
    Done,
    /// The gate is unmet. The scheduler re-prompts once within the attempt
    /// (spec §6.5) and, if still blocked, pauses the run `blocked_gate`.
    Blocked,
    /// The `approval` gate: no predicate the engine can decide — the run pauses
    /// `approval` and a human resolves it via `wf_approve`.
    AwaitingApproval,
}

/// A gate evaluation: the outcome plus a human-readable reason. The reason is
/// journaled on **every** `gate_evaluated` event (success included, spec §6.3
/// step 6) and, on `Blocked`, is quoted back to the agent in the re-prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateResult {
    pub outcome: GateOutcome,
    pub reason: String,
}

impl GateResult {
    fn done(reason: impl Into<String>) -> Self {
        Self {
            outcome: GateOutcome::Done,
            reason: reason.into(),
        }
    }
    fn blocked(reason: impl Into<String>) -> Self {
        Self {
            outcome: GateOutcome::Blocked,
            reason: reason.into(),
        }
    }
}

/// The result of running the project's tests for the `tests` gate (spec §9.4).
/// Produced by `workflow::tests_gate` (which does the I/O) and handed to
/// [`evaluate`] as a fact so gate evaluation stays pure and unit-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestsOutcome {
    /// The test command exited 0.
    Passed,
    /// The test command exited non-zero. `tail` is the last lines of output
    /// (spec §9.4), quoted into the `gate_evaluated` payload and the re-prompt.
    Failed { tail: String },
    /// The test command did not finish within `tests_timeout_secs`.
    TimedOut { tail: String },
    /// The project's setup/install command failed, so tests never ran — a
    /// distinct cause from failing tests (spec §9.4).
    SetupFailed { tail: String },
    /// No test command could be resolved (no override, nothing detected). The
    /// `tests` gate can verify nothing, so it blocks with a named cause rather
    /// than falling back to the agent's self-reported verdict — a step that
    /// wants self-report declares `gate: verdict` explicitly (spec §9.4).
    NoCommand,
}

/// Facts the caller gathers before asking for a gate decision. Everything here
/// is already-resolved data — this module performs no I/O.
#[derive(Debug, Default, Clone)]
pub struct GateInputs<'a> {
    /// The parsed blackboard verdict for this attempt, or `None` when the file
    /// was missing or malformed (both treated as unmet — spec §8.3).
    pub verdict: Option<&'a Verdict>,
    /// Why the verdict is absent, for a precise `Blocked` reason (e.g. the JSON
    /// parse error). Ignored when `verdict` is `Some`.
    pub verdict_error: Option<&'a str>,
    /// HEAD at the fork point, before the agent's turn (spec §6.3 step 3).
    pub head_start: Option<&'a str>,
    /// HEAD after the turn's boundary work — for the `commit` gate.
    pub head_end: Option<&'a str>,
    /// Whether the `artifact` gate's declared path exists in the worktree.
    pub artifact_present: bool,
    /// Whether a human has approved (the `approval` gate). `false` on the first
    /// evaluation → `AwaitingApproval`; the `wf_approve` path re-evaluates with
    /// `true`.
    pub approved: bool,
    /// The result of running the project's tests (the `tests` gate and an
    /// `approval` gate's `require: [tests]` only). `None` for every other gate;
    /// a `tests` gate with `NoCommand` blocks as unverifiable (spec §9.4).
    pub tests: Option<&'a TestsOutcome>,
}

/// Evaluate `gate` against `inputs`. Pure and total — no panics, no I/O.
pub fn evaluate(gate: &Gate, inputs: &GateInputs) -> GateResult {
    match gate {
        Gate::Verdict => evaluate_verdict(inputs),
        Gate::Commit => evaluate_commit(inputs),
        Gate::Artifact { path } => evaluate_artifact(path, inputs),
        Gate::Approval { require, artifact } => {
            evaluate_approval(require, artifact.as_deref(), inputs)
        }
        Gate::Tests => evaluate_tests(inputs),
    }
}

fn evaluate_verdict(inputs: &GateInputs) -> GateResult {
    match inputs.verdict {
        Some(v) => match v.result {
            VerdictResult::Done => GateResult::done("verdict.json result is \"done\""),
            VerdictResult::Revise => GateResult::blocked(format!(
                "verdict.json result is \"revise\": {}",
                summary_or(v, "no summary")
            )),
            VerdictResult::Blocked => GateResult::blocked(format!(
                "verdict.json result is \"blocked\": {}",
                summary_or(v, "no summary")
            )),
        },
        None => GateResult::blocked(match inputs.verdict_error {
            Some(e) => format!("verdict.json unreadable: {e}"),
            None => "verdict.json not written yet".to_string(),
        }),
    }
}

fn evaluate_commit(inputs: &GateInputs) -> GateResult {
    match (inputs.head_start, inputs.head_end) {
        (Some(start), Some(end)) if start != end => {
            GateResult::done(format!("HEAD advanced {} → {}", short(start), short(end)))
        }
        (Some(_), Some(_)) => {
            GateResult::blocked("no commit was made this attempt (HEAD unchanged)")
        }
        // A missing HEAD means the worktree facts couldn't be read; treat as
        // unmet rather than asserting completion.
        _ => GateResult::blocked("could not read worktree HEAD to check for a commit"),
    }
}

fn evaluate_artifact(path: &str, inputs: &GateInputs) -> GateResult {
    if inputs.artifact_present {
        GateResult::done(format!("required artifact `{path}` exists"))
    } else {
        GateResult::blocked(format!("required artifact `{path}` does not exist yet"))
    }
}

fn evaluate_approval(
    require: &[Require],
    artifact: Option<&str>,
    inputs: &GateInputs,
) -> GateResult {
    // A declared review artifact (spec §9) is a deterministic prerequisite,
    // exactly like `require: [tests]`: the pause is unreachable until the file
    // exists, and a missing file blocks with the same reason the `artifact`
    // gate would give — never `AwaitingApproval` over a document that isn't
    // there for the human to read.
    if let Some(path) = artifact {
        let presence = evaluate_artifact(path, inputs);
        if presence.outcome == GateOutcome::Blocked {
            return presence;
        }
    }
    // `require: [tests]` (spec §9): the deterministic gate is evaluated first, so
    // the approval pause is unreachable while tests are red — a failing/timed-out/
    // setup-failed run blocks exactly like a `tests` gate, quoting the same reason
    // (and output tail) so the re-prompt is identical. Unlike the bare `tests`
    // gate, no resolvable test command does NOT block here: the approval gate's
    // decision is a *human*, itself a verifiable condition (spec §9.4), so an
    // unverifiable-tests step still escalates to the human. We only short-circuit
    // when the step's own verdict says "revise"/"blocked" — a self-reported
    // not-done step must not reach the human as ready-to-approve. A step with no
    // verdict at all still falls through to approval — the approval gate never
    // demands a verdict, and blocking on a missing file would strand the step in a
    // re-prompt loop tests can't satisfy.
    if require.contains(&Require::Tests) {
        if let Some(reason) = tests_block_reason(inputs.tests) {
            return GateResult::blocked(reason);
        }
        if matches!(inputs.tests, Some(TestsOutcome::NoCommand)) && inputs.verdict.is_some() {
            let self_reported = evaluate_verdict(inputs);
            if self_reported.outcome == GateOutcome::Blocked {
                return self_reported;
            }
        }
    }
    if inputs.approved {
        GateResult::done("approved by a human")
    } else {
        GateResult {
            outcome: GateOutcome::AwaitingApproval,
            reason: "waiting for human approval".to_string(),
        }
    }
}

fn evaluate_tests(inputs: &GateInputs) -> GateResult {
    if let Some(reason) = tests_block_reason(inputs.tests) {
        return GateResult::blocked(reason);
    }
    match inputs.tests {
        Some(TestsOutcome::Passed) => GateResult::done("project tests passed"),
        // No test command resolvable → the gate has verified *nothing*, so it must
        // NOT be satisfiable by the agent's self-reported verdict. Routing to the
        // verdict here would silently turn `gate: tests` into `gate: verdict` —
        // the exact "done only when a verifiable condition holds, not when the
        // agent says so" guarantee this gate exists to keep. Block with a named
        // cause instead (every pause names its cause, spec §6/§9.4); a step that
        // genuinely wants agent self-report declares `gate: verdict` explicitly.
        Some(TestsOutcome::NoCommand) => GateResult::blocked(
            "tests gate: no test command resolved — cannot verify \
             (configure a test command via `run.test`, or use `gate: verdict` \
             for agent self-report)",
        ),
        // `None` means the caller skipped the runner for a `tests` gate — a bug;
        // block rather than assert an unverified completion. The failing outcomes
        // returned via the guard above, so they never reach this arm, which exists
        // only to keep the match exhaustive without panicking (the module is pure
        // and total).
        _ => GateResult::blocked("tests gate: tests were not run — cannot verify"),
    }
}

/// The `Blocked` reason for a *failing* tests outcome (red / timed out / setup
/// failed), or `None` when tests passed, weren't run, or resolved to no command.
/// Shared by the `tests` gate and an `approval` gate's `require: [tests]` so both
/// speak the identical failure reason and output tail (spec §9.4).
fn tests_block_reason(tests: Option<&TestsOutcome>) -> Option<String> {
    match tests {
        Some(TestsOutcome::Failed { tail }) => Some(with_tail("project tests failed", tail)),
        Some(TestsOutcome::TimedOut { tail }) => {
            Some(with_tail("project tests timed out before finishing", tail))
        }
        Some(TestsOutcome::SetupFailed { tail }) => Some(with_tail(
            "project setup command failed (tests not run)",
            tail,
        )),
        _ => None,
    }
}

/// How much of the runner's output tail rides in a gate reason. The verifier
/// keeps 100 lines, which is right for a log pane but not for a reason string:
/// this one becomes the run's `error` (the monitor's failure banner) and the
/// re-prompt, and a per-test runner fills 100 lines with passes. Every runner
/// prints its failure summary last, so the final lines are the signal.
const REASON_TAIL_LINES: usize = 20;

/// Compose a gate reason from a headline plus an optional output tail. The tail
/// rides in the reason so it reaches both the `gate_evaluated` journal event and
/// the re-prompt (spec §9.4) through the existing blocked-reason plumbing.
fn with_tail(headline: &str, tail: &str) -> String {
    let tail = tail.trim_end();
    if tail.is_empty() {
        return headline.to_string();
    }
    let lines: Vec<&str> = tail.lines().collect();
    let start = lines.len().saturating_sub(REASON_TAIL_LINES);
    let kept = lines[start..].join("\n");
    if start > 0 {
        format!("{headline} (last {REASON_TAIL_LINES} lines):\n{kept}")
    } else {
        format!("{headline}:\n{kept}")
    }
}

fn summary_or<'a>(v: &'a Verdict, fallback: &'a str) -> &'a str {
    if v.summary.trim().is_empty() {
        fallback
    } else {
        v.summary.trim()
    }
}

/// Abbreviate a SHA for log/journal readability without assuming a 40-char len.
fn short(sha: &str) -> &str {
    if sha.len() >= 8 {
        &sha[..8]
    } else {
        sha
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(result: VerdictResult, summary: &str) -> Verdict {
        Verdict {
            result,
            summary: summary.to_string(),
            detail: None,
            target: None,
        }
    }

    #[test]
    fn verdict_done_passes() {
        let v = verdict(VerdictResult::Done, "shipped");
        let r = evaluate(
            &Gate::Verdict,
            &GateInputs {
                verdict: Some(&v),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Done);
    }

    #[test]
    fn verdict_revise_blocks_with_summary() {
        let v = verdict(VerdictResult::Revise, "fix the off-by-one");
        let r = evaluate(
            &Gate::Verdict,
            &GateInputs {
                verdict: Some(&v),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("off-by-one"), "reason: {}", r.reason);
    }

    #[test]
    fn verdict_missing_blocks() {
        let r = evaluate(&Gate::Verdict, &GateInputs::default());
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("not written"));
    }

    #[test]
    fn verdict_malformed_quotes_error() {
        let r = evaluate(
            &Gate::Verdict,
            &GateInputs {
                verdict_error: Some("expected `,` at line 3"),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("line 3"), "reason: {}", r.reason);
    }

    #[test]
    fn commit_gate_detects_moved_head() {
        let done = evaluate(
            &Gate::Commit,
            &GateInputs {
                head_start: Some("aaaaaaaaaaaa"),
                head_end: Some("bbbbbbbbbbbb"),
                ..Default::default()
            },
        );
        assert_eq!(done.outcome, GateOutcome::Done);

        let unchanged = evaluate(
            &Gate::Commit,
            &GateInputs {
                head_start: Some("aaaaaaaaaaaa"),
                head_end: Some("aaaaaaaaaaaa"),
                ..Default::default()
            },
        );
        assert_eq!(unchanged.outcome, GateOutcome::Blocked);
    }

    #[test]
    fn commit_gate_unreadable_head_blocks() {
        let r = evaluate(
            &Gate::Commit,
            &GateInputs {
                head_start: Some("aaaa"),
                head_end: None,
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Blocked);
    }

    #[test]
    fn artifact_gate_checks_presence() {
        let present = evaluate(
            &Gate::Artifact {
                path: "PLAN.md".into(),
            },
            &GateInputs {
                artifact_present: true,
                ..Default::default()
            },
        );
        assert_eq!(present.outcome, GateOutcome::Done);

        let absent = evaluate(
            &Gate::Artifact {
                path: "PLAN.md".into(),
            },
            &GateInputs::default(),
        );
        assert_eq!(absent.outcome, GateOutcome::Blocked);
        assert!(absent.reason.contains("PLAN.md"));
    }

    #[test]
    fn approval_gate_awaits_then_passes() {
        let bare = Gate::Approval {
            require: vec![],
            artifact: None,
        };
        let waiting = evaluate(&bare, &GateInputs::default());
        assert_eq!(waiting.outcome, GateOutcome::AwaitingApproval);

        let approved = evaluate(
            &bare,
            &GateInputs {
                approved: true,
                ..Default::default()
            },
        );
        assert_eq!(approved.outcome, GateOutcome::Done);
    }

    #[test]
    fn approval_with_missing_artifact_blocks_not_awaits() {
        // An approval gate's declared artifact is a prerequisite exactly like an
        // unmet `require: [tests]` (spec §9): while the file is missing the human
        // pause is unreachable — the step blocks (re-prompt) with a reason naming
        // the path, never `AwaitingApproval` over a document that doesn't exist.
        let gate = Gate::Approval {
            require: vec![],
            artifact: Some("PLAN.md".into()),
        };
        let r = evaluate(&gate, &GateInputs::default());
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("PLAN.md"), "reason: {}", r.reason);

        // Present → the ordinary approval pause; approved → done.
        let waiting = evaluate(
            &gate,
            &GateInputs {
                artifact_present: true,
                ..Default::default()
            },
        );
        assert_eq!(waiting.outcome, GateOutcome::AwaitingApproval);
        let approved = evaluate(
            &gate,
            &GateInputs {
                artifact_present: true,
                approved: true,
                ..Default::default()
            },
        );
        assert_eq!(approved.outcome, GateOutcome::Done);
    }

    #[test]
    fn approval_missing_artifact_blocks_even_when_approved_and_green() {
        // The artifact prerequisite is checked before everything else: neither a
        // prior approval nor green tests can carry the gate past a missing file.
        let gate = Gate::Approval {
            require: vec![Require::Tests],
            artifact: Some("PLAN.md".into()),
        };
        let r = evaluate(
            &gate,
            &GateInputs {
                approved: true,
                tests: Some(&TestsOutcome::Passed),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("PLAN.md"), "reason: {}", r.reason);
    }

    #[test]
    fn approval_require_tests_blocks_before_asking_a_human() {
        // With `require: [tests]`, a red test run must block (and quote the tail)
        // rather than reach the human-approval pause — the deterministic gate is
        // evaluated first (spec §9).
        let gate = Gate::Approval {
            require: vec![Require::Tests],
            artifact: None,
        };
        let failed = TestsOutcome::Failed {
            tail: "FAIL src/x.test.ts\n  ✕ adds".into(),
        };
        let r = evaluate(
            &gate,
            &GateInputs {
                tests: Some(&failed),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("adds"), "reason: {}", r.reason);
    }

    #[test]
    fn approval_require_tests_awaits_human_once_green() {
        // Passing tests (or no resolvable command) must fall through to the human
        // pause — the engine never blocks on tests it can't or didn't fail to run.
        let gate = Gate::Approval {
            require: vec![Require::Tests],
            artifact: None,
        };
        for outcome in [TestsOutcome::Passed, TestsOutcome::NoCommand] {
            let r = evaluate(
                &gate,
                &GateInputs {
                    tests: Some(&outcome),
                    ..Default::default()
                },
            );
            assert_eq!(
                r.outcome,
                GateOutcome::AwaitingApproval,
                "outcome {outcome:?} should await the human"
            );
        }
    }

    #[test]
    fn approval_require_tests_no_command_blocks_a_self_reported_not_done() {
        // With no resolvable test command the approval gate still escalates to the
        // human (itself a verifiable condition, spec §9.4) — but a step whose own
        // verdict says "revise"/"blocked" must not reach the human as ready-to-
        // approve, so it blocks first.
        let gate = Gate::Approval {
            require: vec![Require::Tests],
            artifact: None,
        };
        let v = verdict(VerdictResult::Revise, "flaky assertion");
        let r = evaluate(
            &gate,
            &GateInputs {
                tests: Some(&TestsOutcome::NoCommand),
                verdict: Some(&v),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("flaky assertion"), "reason: {}", r.reason);

        // A "done" verdict falls through to the human as usual.
        let v = verdict(VerdictResult::Done, "shipped");
        let r = evaluate(
            &gate,
            &GateInputs {
                tests: Some(&TestsOutcome::NoCommand),
                verdict: Some(&v),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::AwaitingApproval);
    }

    #[test]
    fn tests_gate_passes_when_tests_pass() {
        let outcome = TestsOutcome::Passed;
        let r = evaluate(
            &Gate::Tests,
            &GateInputs {
                tests: Some(&outcome),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Done);
    }

    #[test]
    fn tests_gate_blocks_with_output_tail_on_failure() {
        let outcome = TestsOutcome::Failed {
            tail: "FAIL src/foo.test.ts\n  ✕ adds numbers".into(),
        };
        let r = evaluate(
            &Gate::Tests,
            &GateInputs {
                tests: Some(&outcome),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("adds numbers"), "reason: {}", r.reason);
    }

    #[test]
    fn tests_gate_distinguishes_timeout_and_setup_failure() {
        let timed = TestsOutcome::TimedOut {
            tail: String::new(),
        };
        let r = evaluate(
            &Gate::Tests,
            &GateInputs {
                tests: Some(&timed),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("timed out"), "reason: {}", r.reason);

        let setup = TestsOutcome::SetupFailed {
            tail: "npm ERR! missing script: build".into(),
        };
        let r = evaluate(
            &Gate::Tests,
            &GateInputs {
                tests: Some(&setup),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("setup"), "reason: {}", r.reason);
    }

    #[test]
    fn tests_gate_no_command_blocks_and_never_self_reports() {
        // No resolvable test command → the gate verified nothing, so it blocks
        // with a named cause even when the agent self-reported "done" (spec §9.4).
        // Otherwise `gate: tests` would silently collapse into `gate: verdict`.
        let none = TestsOutcome::NoCommand;
        let v = verdict(VerdictResult::Done, "ok");
        let blocked = evaluate(
            &Gate::Tests,
            &GateInputs {
                tests: Some(&none),
                verdict: Some(&v),
                ..Default::default()
            },
        );
        assert_eq!(blocked.outcome, GateOutcome::Blocked);
        assert!(
            blocked.reason.contains("no test command resolved"),
            "reason should name the cause: {}",
            blocked.reason
        );

        // With no verdict at all it likewise blocks (never asserts completion).
        let blocked = evaluate(
            &Gate::Tests,
            &GateInputs {
                tests: Some(&none),
                ..Default::default()
            },
        );
        assert_eq!(blocked.outcome, GateOutcome::Blocked);

        // The legitimate escape hatch is untouched: the *same* self-reported
        // "done" verdict on an explicit `verdict` gate still completes the step.
        let done = evaluate(
            &Gate::Verdict,
            &GateInputs {
                verdict: Some(&v),
                ..Default::default()
            },
        );
        assert_eq!(done.outcome, GateOutcome::Done);
    }

    #[test]
    fn tests_gate_missing_run_blocks() {
        // A `tests` gate whose runner was never consulted (`tests: None`) is a
        // caller bug; it must block rather than assert an unverified completion.
        let r = evaluate(&Gate::Tests, &GateInputs::default());
        assert_eq!(r.outcome, GateOutcome::Blocked);
        assert!(r.reason.contains("cannot verify"), "reason: {}", r.reason);
    }
}
