//! Tests-gate execution (spec §9.4) — a thin adapter over [`crate::verify`].
//!
//! The `tests` gate runs the project's install + test commands, bounded and
//! sandboxed in the step worktree, and reports a [`gates::TestsOutcome`] the
//! pure `gates::evaluate` turns into done/blocked. The actual running (command
//! resolution, seatbelt sandboxing, output-tail bounding, setup-once caching) is
//! the shared [`crate::verify::Verifier`] primitive; this module only:
//!
//! * exposes the [`TestRunner`] trait so `attempt.rs` stays unit-testable with a
//!   scripted mock (the same seam `AgentDriver` provides), and
//! * maps a *tests-only* [`crate::verify::VerificationReport`] back to the
//!   `TestsOutcome` the gate has always spoken.
//!
//! No test command resolvable → [`TestsOutcome::NoCommand`], and the gate
//! degrades to `verdict` (§9.4). Off-macOS (no `sandbox-exec`) degrades the same
//! way rather than running a repo-derived command unsandboxed.

use std::path::Path;

use crate::error::Result;
use crate::verify::{CheckOutcome, VerificationReport, Verifier};

use super::driver::BoxFuture;
use super::gates::TestsOutcome;

/// Resolves and runs the `tests` gate. Behind a trait so the attempt lifecycle
/// is unit-testable with a scripted mock (`AgentDriver`'s pattern).
pub trait TestRunner: Send + Sync {
    /// Resolve and run the project's tests in `worktree`, returning the outcome.
    fn run_tests<'a>(&'a self, worktree: &'a Path) -> BoxFuture<'a, TestsOutcome>;
}

/// The production runner: a [`Verifier`] configured with the project's
/// `run.test` / `run.install` overrides, whose tests-only report is mapped back
/// to a [`TestsOutcome`].
pub struct SandboxTestRunner {
    verifier: Verifier,
}

impl SandboxTestRunner {
    pub fn new(
        test_override: Option<String>,
        setup_override: Option<String>,
        timeout_secs: u64,
    ) -> Result<Self> {
        // The tests gate never lints; `lint_override` is `None`.
        let verifier = Verifier::new(test_override, setup_override, None, timeout_secs)?;
        Ok(Self { verifier })
    }
}

impl TestRunner for SandboxTestRunner {
    fn run_tests<'a>(&'a self, worktree: &'a Path) -> BoxFuture<'a, TestsOutcome> {
        Box::pin(async move {
            // The gate runs under the Run-panel seatbelt profile, which needs
            // macOS `sandbox-exec`. Where it isn't present we can't safely run a
            // repo-derived command, so degrade to the verdict gate (spec §9.4)
            // rather than fail the step for the wrong reason.
            if !sandbox_available() {
                tracing::warn!(
                    "tests gate: `{}` unavailable on this host; degrading to the verdict gate",
                    crate::sandbox::SANDBOX_EXEC
                );
                return TestsOutcome::NoCommand;
            }
            let report = self.verifier.verify_tests_only(worktree).await;
            map_tests_outcome(&report)
        })
    }
}

/// Whether the seatbelt sandbox binary the tests gate runs under is present
/// (macOS). Elsewhere the gate degrades to the verdict gate rather than fail.
fn sandbox_available() -> bool {
    Path::new(crate::sandbox::SANDBOX_EXEC).exists()
}

/// Map a tests-only [`VerificationReport`] back to the gate's [`TestsOutcome`].
///
/// A failed / timed-out / unrunnable install blocks the tests before they run —
/// a distinct cause (`SetupFailed`) from failing tests (spec §9.4). Otherwise
/// the `test` check drives the outcome: a `Skipped` test (nothing resolved)
/// degrades to `NoCommand` → the verdict gate.
fn map_tests_outcome(report: &VerificationReport) -> TestsOutcome {
    if let Some(install) = report.check("install") {
        if matches!(
            install.outcome,
            CheckOutcome::Failed | CheckOutcome::TimedOut | CheckOutcome::SetupFailed
        ) {
            return TestsOutcome::SetupFailed {
                tail: join(&install.tail),
            };
        }
    }
    match report.check("test") {
        Some(t) => match t.outcome {
            CheckOutcome::Passed => TestsOutcome::Passed,
            CheckOutcome::Failed => TestsOutcome::Failed { tail: join(&t.tail) },
            CheckOutcome::TimedOut => TestsOutcome::TimedOut { tail: join(&t.tail) },
            // Install was clean above, so a `SetupFailed` test here can only mean
            // an unresolved prerequisite; treat conservatively as no command.
            CheckOutcome::SetupFailed | CheckOutcome::Skipped => TestsOutcome::NoCommand,
        },
        None => TestsOutcome::NoCommand,
    }
}

/// Join an output tail back into the newline-delimited string the gate reason
/// and journal payload carry.
fn join(tail: &[String]) -> String {
    tail.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::CheckResult;

    fn check(name: &str, outcome: CheckOutcome, tail: &[&str]) -> CheckResult {
        CheckResult {
            name: name.to_string(),
            command: "x".to_string(),
            outcome,
            duration_ms: 1,
            tail: tail.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn maps_passing_tests() {
        let report = VerificationReport {
            checks: vec![
                check("install", CheckOutcome::Passed, &[]),
                check("test", CheckOutcome::Passed, &[]),
            ],
        };
        assert_eq!(map_tests_outcome(&report), TestsOutcome::Passed);
    }

    #[test]
    fn maps_failing_tests_with_tail() {
        let report = VerificationReport {
            checks: vec![check("test", CheckOutcome::Failed, &["FAIL", "  x adds"])],
        };
        match map_tests_outcome(&report) {
            TestsOutcome::Failed { tail } => assert!(tail.contains("adds"), "tail: {tail}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn install_failure_maps_to_setup_failed() {
        let report = VerificationReport {
            checks: vec![
                check("install", CheckOutcome::Failed, &["npm ERR!"]),
                check("test", CheckOutcome::SetupFailed, &[]),
            ],
        };
        match map_tests_outcome(&report) {
            TestsOutcome::SetupFailed { tail } => assert!(tail.contains("npm ERR!")),
            other => panic!("expected SetupFailed, got {other:?}"),
        }
    }

    #[test]
    fn install_timeout_maps_to_setup_failed() {
        let report = VerificationReport {
            checks: vec![
                check("install", CheckOutcome::TimedOut, &["setup command exceeded"]),
                check("test", CheckOutcome::SetupFailed, &[]),
            ],
        };
        assert!(matches!(
            map_tests_outcome(&report),
            TestsOutcome::SetupFailed { .. }
        ));
    }

    #[test]
    fn test_timeout_maps_to_timed_out() {
        let report = VerificationReport {
            checks: vec![
                check("install", CheckOutcome::Passed, &[]),
                check("test", CheckOutcome::TimedOut, &["test command exceeded"]),
            ],
        };
        assert!(matches!(
            map_tests_outcome(&report),
            TestsOutcome::TimedOut { .. }
        ));
    }

    #[test]
    fn skipped_test_degrades_to_no_command() {
        let report = VerificationReport {
            checks: vec![check("test", CheckOutcome::Skipped, &[])],
        };
        assert_eq!(map_tests_outcome(&report), TestsOutcome::NoCommand);
    }
}
