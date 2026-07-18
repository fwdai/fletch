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

/** One changed file in an approval gate's ferried diff. */
export interface GateDiffFile {
  path: string;
  additions: number;
  deletions: number;
}

/** The ferried diff (vs the run base) summarized for review. */
export interface GateDiff {
  additions: number;
  deletions: number;
  files: GateDiffFile[];
}

/** Budget spent vs cap at an approval pause. `tokens_cap === null` means the run
 *  has no token cap; a `tokens_spent` of 0 with no cap should render as "unknown"
 *  (some providers don't report token usage — driver.rs). */
export interface GateBudget {
  turns_spent: number;
  turns_cap: number;
  tokens_spent: number;
  tokens_cap: number | null;
  wall_ms_spent: number;
  wall_clock_cap_mins: number;
}

/** A reviewer step's `verdict.json` summary carried in the evidence. */
export interface GateVerdict {
  result: string;
  summary: string;
  detail: string | null;
  target: string | null;
}

/** The review evidence assembled when an approval gate pauses a run (the Rust
 *  `gate_evidence` event payload, spec §9): verification, the ferried diff vs the
 *  run base, budget spend, and the step's verdict. `verification` is null when the
 *  host couldn't build a verifier; its checks are all `skipped` when the project
 *  configures no commands. `base_sha`/`head_sha` feed `api.wfRunDiff`. */
export interface GateEvidence {
  base_sha: string;
  head_sha: string;
  verification: VerificationReport | null;
  diff: GateDiff;
  budget: GateBudget;
  verdict: GateVerdict | null;
}
