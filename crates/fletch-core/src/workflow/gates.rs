//! Gate evaluation — the deterministic predicate that decides a step is done
//! (spec §9). Every gate is a **pure** function of already-gathered facts: the
//! kernel reads the blackboard verdict, then asks this module for the decision,
//! so the semantics and reason wording live in one place and are trivially
//! unit-testable and journalable.

use super::blackboard::{Verdict, VerdictResult};
use super::spec::Gate;

/// Whether the gate is met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// The gate is satisfied; the step completes and the run advances.
    Done,
    /// The gate is unmet; the run fails with the reason.
    Blocked,
}

/// A gate evaluation: the outcome plus a human-readable reason, journaled on
/// every `gate_evaluated` event and, on `Blocked`, carried onto the run's error.
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

/// Facts the caller gathers before asking for a gate decision. Everything here
/// is already-resolved data — this module performs no I/O.
#[derive(Debug, Default, Clone)]
pub struct GateInputs<'a> {
    /// The parsed blackboard verdict for this step, or `None` when the file was
    /// missing or malformed (both treated as unmet — spec §8.3).
    pub verdict: Option<&'a Verdict>,
    /// Why the verdict is absent, for a precise `Blocked` reason (e.g. the JSON
    /// parse error). Ignored when `verdict` is `Some`.
    pub verdict_error: Option<&'a str>,
}

/// Evaluate `gate` against `inputs`. Pure and total — no panics, no I/O.
pub fn evaluate(gate: &Gate, inputs: &GateInputs) -> GateResult {
    match gate {
        Gate::Verdict => evaluate_verdict(inputs),
        // The boundary commit records whatever the turn produced, including
        // nothing, so the turn having ended is the whole predicate.
        Gate::Commit => GateResult::done("the turn ended; its work is in the boundary commit"),
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

fn summary_or<'a>(v: &'a Verdict, fallback: &'a str) -> &'a str {
    if v.summary.trim().is_empty() {
        fallback
    } else {
        v.summary.trim()
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
    fn commit_gate_is_met_by_the_turn_ending_whatever_the_verdict() {
        assert_eq!(
            evaluate(&Gate::Commit, &GateInputs::default()).outcome,
            GateOutcome::Done
        );
        // A commit gate never reads the verdict, so a "revise" left on the
        // blackboard does not hold the step back.
        let v = verdict(VerdictResult::Revise, "not me");
        let r = evaluate(
            &Gate::Commit,
            &GateInputs {
                verdict: Some(&v),
                ..Default::default()
            },
        );
        assert_eq!(r.outcome, GateOutcome::Done);
    }
}
