import type { AgentStatus, GitState, PrChecks, PrState } from "@/api";
import type { IconName } from "@/components/Icon";

/** The four glanceable states the capsule dot collapses to. Distinct from
 *  AgentStatus: `waiting` is derived (running + a pending question), and
 *  spawning/stopped fold into running/idle. */
export type DotStatus = "running" | "waiting" | "idle" | "error";

export const STATUS_LABEL: Record<DotStatus, string> = {
  running: "Working",
  waiting: "Waiting for input",
  idle: "Idle",
  error: "Failed",
};

/** Map the agent's run status (+ a pending question) to the capsule dot.
 *  `awaiting` mirrors the sidebar row: running with an unanswered prompt is
 *  "your court", not "still working". */
export function dotStatus(status: AgentStatus, awaiting: boolean): DotStatus {
  if (awaiting) return "waiting";
  if (status === "running" || status === "spawning") return "running";
  if (status === "error") return "error";
  return "idle";
}

export type PrBadge = "open" | "draft" | "conflicts" | "merged" | "closed";

export const PR_META: Record<PrBadge, { label: string; icon: IconName; cls: string }> = {
  open: { label: "Open", icon: "branch", cls: "open" },
  draft: { label: "Draft", icon: "branch", cls: "draft" },
  conflicts: { label: "Conflicts", icon: "merge", cls: "conflicts" },
  merged: { label: "Merged", icon: "merge", cls: "merged" },
  closed: { label: "Closed", icon: "branch", cls: "draft" },
};

/** Refine a remote PR into its tinted badge state using local conflict markers
 *  and GitHub's merge gate (draft/dirty) — the same signals the Git panel uses. */
export function prBadge(pr: PrState, git: GitState | null, checks: PrChecks | null): PrBadge {
  if (pr.state === "merged") return "merged";
  if (pr.state === "closed") return "closed";
  const conflicted =
    git?.files.some((f) => f.kind === "conflicted") || checks?.merge_state === "dirty";
  if (conflicted) return "conflicts";
  if (checks?.merge_state === "draft") return "draft";
  return "open";
}

/** "owner/repo" from a github remote URL (https or ssh form), else null. */
export function repoSlug(remoteUrl: string | null | undefined): string | null {
  const m = remoteUrl?.match(/github\.com[/:]([^/]+\/[^/\s]+?)(?:\.git)?$/);
  return m ? m[1] : null;
}
