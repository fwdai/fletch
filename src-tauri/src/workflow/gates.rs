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
//! pure module. When no test command resolves the gate degrades to `verdict`.

use super::blackboard::{Verdict, VerdictResult};
use super::spec::Gate;

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
    /// gate degrades to `verdict` with a journaled warning (spec §9.4).
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
    /// The result of running the project's tests (the `tests` gate only). `None`
    /// for every other gate; a `tests` gate with `NoCommand` degrades to the
    /// verdict facts above (spec §9.4).
    pub tests: Option<&'a TestsOutcome>,
}

/// Evaluate `gate` against `inputs`. Pure and total — no panics, no I/O.
pub fn evaluate(gate: &Gate, inputs: &GateInputs) -> GateResult {
    match gate {
        Gate::Verdict => evaluate_verdict(inputs),
        Gate::Commit => evaluate_commit(inputs),
        Gate::Artifact { path } => evaluate_artifact(path, inputs),
        Gate::Approval => evaluate_approval(inputs),
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

fn evaluate_approval(inputs: &GateInputs) -> GateResult {
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
    match inputs.tests {
        Some(TestsOutcome::Passed) => GateResult::done("project tests passed"),
        Some(TestsOutcome::Failed { tail }) => {
            GateResult::blocked(with_tail("project tests failed", tail))
        }
        Some(TestsOutcome::TimedOut { tail }) => {
            GateResult::blocked(with_tail("project tests timed out before finishing", tail))
        }
        Some(TestsOutcome::SetupFailed { tail }) => GateResult::blocked(with_tail(
            "project setup command failed (tests not run)",
            tail,
        )),
        // No test command resolvable → degrade to the verdict gate (spec §9.4).
        // `attempt.rs` journals the degrade warning; here we just read the
        // verdict facts the caller always gathers.
        Some(TestsOutcome::NoCommand) | None => evaluate_verdict(inputs),
    }
}

/// Compose a gate reason from a headline plus an optional output tail. The tail
/// rides in the reason so it reaches both the `gate_evaluated` journal event and
/// the re-prompt (spec §9.4) through the existing blocked-reason plumbing.
fn with_tail(headline: &str, tail: &str) -> String {
    let tail = tail.trim_end();
    if tail.is_empty() {
        headline.to_string()
    } else {
        format!("{headline}:\n{tail}")
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
        let waiting = evaluate(&Gate::Approval, &GateInputs::default());
        assert_eq!(waiting.outcome, GateOutcome::AwaitingApproval);

        let approved = evaluate(
            &Gate::Approval,
            &GateInputs {
                approved: true,
                ..Default::default()
            },
        );
        assert_eq!(approved.outcome, GateOutcome::Done);
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
    fn tests_gate_no_command_degrades_to_verdict() {
        let none = TestsOutcome::NoCommand;
        // With a done verdict present the degraded gate passes …
        let v = verdict(VerdictResult::Done, "ok");
        let r = evaluate(
            &Gate::Tests,
            &GateInputs {
                tests: Some(&none),
                verdict: Some(&v),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Done);

        // … and without one it blocks exactly like a verdict gate.
        let r = evaluate(
            &Gate::Tests,
            &GateInputs {
                tests: Some(&none),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Blocked);
    }
}
