import type { CyclePhase, GiveUpReason } from "@/autopilot";
import type { DelegationKind, DelegationPhase } from "@/delegation";

export interface DiffStats {
  additions: number;
  deletions: number;
}

export type StatusKind = "modified" | "added" | "deleted" | "renamed" | "untracked" | "conflicted";

export interface FileStatus {
  path: string;
  kind: StatusKind;
  staged: boolean;
  additions: number;
  deletions: number;
}

/** Compact projection of GitState used by the app-wide bulk poll —
 *  enough for sidebar shortstats and tab badges without shipping every
 *  agent's file list over IPC. */
export interface ShortStats {
  additions: number;
  deletions: number;
  file_count: number;
}

/** Advisory fleet-wide git metadata for one checkout, keyed by `checkoutKey` in the
 *  bulk `getAllGitMeta` reply. `behind` (base moved ahead of this checkout) and
 *  `files` (working-tree paths) drive the always-visible staleness chip and the
 *  cross-agent overlap hints. `behind` is null when the base tip can't be
 *  resolved (no GitHub / no fetch yet) — render nothing, never a zero. */
export interface GitMeta {
  base: string;
  behind: number | null;
  files: string[];
}

export interface GitState {
  branch: string;
  parent_branch: string;
  ahead: number;
  behind: number;
  /** Commits on HEAD not yet on the upstream — how many a push would send.
   *  Distinct from `ahead` (measured vs the base branch). */
  unpushed: number;
  files: FileStatus[];
  additions: number;
  deletions: number;
  /** GitHub web base for `origin` (`https://github.com/owner/repo`), or null
   *  when origin is missing / not a github.com remote. Lets the panel link to
   *  a commit or compare view. */
  remote_url?: string | null;
  /** Whether an `origin` remote exists at all (GitHub or not). False = a
   *  local-only repo: push/PR give way to "Publish to GitHub". */
  has_origin: boolean;
  /** HEAD commit SHA, for a single-commit link when one commit is ahead. */
  head_sha?: string | null;
  /** Config keys Fletch refuses to run git over in this checkout. Non-empty
   *  means every other field is a zero-state; the panel offers to remove them.
   *  Absent from hosts that predate the field. */
  blocked_config?: string[];
  /** Linked worktrees inside the checkout — sub-agents working in isolation.
   *  Listed, never status-read. Absent from hosts that predate the field. */
  worktrees?: LinkedWorktree[];
}

/** One linked worktree of a checkout; `branch` is null when it is detached. */
export interface LinkedWorktree {
  path: string;
  branch: string | null;
}

/** One delegation as the host reports it: a row of `get_delegations` (a live
 *  phase, no `notice`), or one step of its life on `delegation:changed`. `done`
 *  and `abandoned` end it; their `notice` is the outcome line to show. An
 *  `abandoned` with no `notice` was dropped because its agent went away. */
export interface DelegationEvent {
  agent_id: string;
  /** The secondary repo it targets; null for the primary. */
  subdir: string | null;
  kind: DelegationKind;
  phase: DelegationPhase | "done" | "abandoned";
  /** Epoch ms when it entered its current phase. */
  started_at: number;
  notice?: string;
}

/** The cycle autopilot has open on a checkout: the rung it handed the agent,
 *  which try that is (1-based), and since when (epoch ms) it has been in the
 *  current phase. */
export interface AutopilotCycle {
  rung: DelegationKind;
  attempt: number;
  phase: CyclePhase;
  since: number;
}

/** One checkout as the host's autopilot sees it: a row of `autopilot_state`,
 *  or one `autopilot:state` event (which replaces the row it names). */
export interface AutopilotCheckout {
  agent_id: string;
  /** The secondary repo; null for the agent's primary. */
  subdir: string | null;
  project_id: string;
  /** `project_enabled && !paused`. */
  enrolled: boolean;
  /** The agent's own pause, from the Git panel's switch. */
  paused: boolean;
  /** The project's switch. */
  project_enabled: boolean;
  cycle: AutopilotCycle | null;
}

/** Both autopilot opt-out lists, whole: part of every snapshot, and the
 *  `autopilot:switches` payload every `autopilot_set` fires. */
export interface AutopilotSwitches {
  /** Projects whose switch is off. Every other project is on. */
  disabled_projects: string[];
  /** Agents paused from the Git panel. */
  paused_agents: string[];
}

/** Autopilot across the whole host: what `autopilot_state` and `autopilot_set`
 *  answer. */
export interface AutopilotSnapshot extends AutopilotSwitches {
  checkouts: AutopilotCheckout[];
}

/** One thing autopilot did: a row of `autopilot_log`, or one `autopilot:event`. */
export interface AutopilotLogEntry {
  id: string;
  agent_id: string;
  subdir: string | null;
  /** Epoch ms. */
  at: number;
  outcome: "dispatch" | "settle" | "retry" | "give-up";
  rung: DelegationKind;
  /** 1-based try of the cycle the row belongs to. */
  attempt: number;
  /** Only on a `give-up`. */
  reason?: GiveUpReason;
}
