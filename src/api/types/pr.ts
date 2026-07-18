export type PrStatus = "open" | "merged" | "closed";

export interface PrState {
  number: number;
  url: string;
  state: PrStatus;
  title: string;
  mergeable: boolean;
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

/** One label on an issue, for the Home inbox's quiet chips. `color` is
 *  GitHub's 6-hex assignment (no leading `#`), used subtly when present. */
export interface IssueLabel {
  name: string;
  color?: string;
}

/** An open GitHub issue for the Home inbox. Carries the body so "Start work"
 *  composes the brief without a second round-trip. */
export interface IssueSummary {
  number: number;
  title: string;
  url: string;
  labels: IssueLabel[];
  assignee?: string;
  /** `updatedAt` as ms-epoch, for the "updated N ago" hint. */
  updated_at?: number;
  body?: string;
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
}

/** Unresolved review threads for a PR — polled on the slow checks cadence. */
export interface PrComments {
  unresolved: PrComment[];
}
