/** One check's terminal outcome in a verification run (Rust
 *  `verify::CheckOutcome`). `skipped` = no command resolved (nothing to run);
 *  `setup_failed` = a prerequisite install failed so the check never ran. */
export type CheckOutcome = "passed" | "failed" | "timed_out" | "setup_failed" | "skipped";

/** One check's result inside a verification run (Rust `verify::CheckResult`). */
export interface CheckResult {
  /** "install" | "test" | "lint" */
  name: string;
  /** The command that ran (or would have); "" when skipped. */
  command: string;
  outcome: CheckOutcome;
  /** Wall-clock duration in ms; 0 when the check didn't run. */
  duration_ms: number;
  /** Last ~100 lines of combined stdout+stderr; empty on success/skip. */
  tail: string[];
}

/** The result of running a project's deterministic checks in a checkout
 *  (Rust `verify::VerificationReport`). */
export interface VerificationReport {
  checks: CheckResult[];
}

/** A turn-end verification finished for an ad-hoc agent (opt-in per project) —
 *  the Mission Control card renders a tests chip from this report. */
export interface VerificationReportEvent {
  agent_id: string;
  report: VerificationReport;
}
