export type PrStatus = "open" | "merged" | "closed";

/** GitHub's coarse `mergeable` verdict — the only merge signal when the richer
 *  `MergeState` (from `mergeStateStatus`) is unavailable. Tri-state on purpose:
 *  GitHub computes mergeability lazily, so `"unknown"` ("not computed yet",
 *  normal for a while after any push) must stay distinct from `"conflicting"`
 *  (a real conflict) — see mergeGate's no-checks fallback. */
export type Mergeable = "mergeable" | "conflicting" | "unknown";

export interface PrState {
  number: number;
  url: string;
  state: PrStatus;
  title: string;
  mergeable: Mergeable;
}

export interface PrStateChangedEvent {
  agent_id: string;
  state: PrState | null;
}

/** Lightweight PR summary for the composer's "#" mention autocomplete. */
export interface PrSummary {
  number: number;
  title: string;
  state: PrStatus;
}

/** GitHub's combined merge gate (`mergeStateStatus`), normalized (spec §6). */
export type MergeState =
  | "clean"
  | "blocked"
  | "unstable"
  | "behind"
  | "dirty"
  | "draft"
  | "has_hooks"
  | "unknown";

/** One CI check, normalized from gh's statusCheckRollup. */
export interface CheckRun {
  name: string;
  status: "queued" | "in_progress" | "completed";
  conclusion: string | null;
  required: boolean;
  url: string | null;
  started_at: string | null;
  completed_at: string | null;
}

/** Rich PR merge-gate + per-check detail. Heavier than PrState — polled on
 *  a slow cadence while a PR is open. */
export interface PrChecks {
  merge_state: MergeState;
  rollup: "none" | "pending" | "passing" | "failing";
  total: number;
  passed: number;
  failed: number;
  pending: number;
  required_failing: string[];
  runs: CheckRun[];
}

/** One unresolved PR review thread, flattened to its root comment. */
export interface PrComment {
  /** The review thread's node id — what the agent's `reply_thread` /
   *  `resolve_thread` ops address. */
  id: string;
  author: string;
  /** Author is a GitHub App / bot (Greptile, CodeRabbit, …). Bots phrase
   *  their comments for an AI already, so the panel inserts them as-is;
   *  human comments get a file/line context wrapper. */
  is_bot: boolean;
  body: string;
  path: string | null;
  line: number | null;
  url: string;
  /** Replies after the root comment. */
  replies: number;
  /** We posted the last comment on this still-open thread: we've had our say and
   *  are waiting on the human. Set when the agent pushed back on a comment and
   *  deliberately left it open — read off GitHub rather than tracked locally, so
   *  it survives a restart and can tell "waiting on them" from "they answered,
   *  engage again". */
  we_replied_last: boolean;
}

/** Unresolved review threads for a PR — polled on the slow checks cadence. */
export interface PrComments {
  unresolved: PrComment[];
}

/** One agent-repo's entry in the app-wide sidebar sweep: PR state, plus the CI
 *  rollup when the PR is open. `checks: null` means "nothing to say this round"
 *  — served from a snapshot, not open, or the lookup degraded — so the store
 *  leaves the last-known tint alone rather than wiping it. */
export interface AgentPrStatus {
  state: PrState;
  checks: PrChecks | null;
}

/** The Git panel's fast-tick PR read — state + CI from one backend pass over
 *  ETag-conditional REST, so both are from the same moment. `checks` is null
 *  when the CI reads didn't resolve (distinct from a rollup of zero checks), so
 *  a transient failure never blanks a passing tint. */
export interface PrLive {
  state: PrState;
  checks: PrChecks | null;
}
