import type { AgentStatus, GitState, PrChecks, PrState } from "@/api";
import type { IconName } from "@/components/Icon";
import type { BadgeVariant } from "@/components/ui/Badge";

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

/** [`dotStatus`] fed straight from store state: `pending` is the agent's
 *  `pendingToolUse` entry, and a question only counts as awaiting while the
 *  agent is actually working. */
export function agentDotStatus(
  status: AgentStatus,
  pending: Record<string, string> | undefined,
): DotStatus {
  const working = status === "running" || status === "spawning";
  return dotStatus(status, working && Object.keys(pending ?? {}).length > 0);
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

/** The capsule badge's tint class once the checkout holds several PRs. A
 *  failing or conflicting PR anywhere in the set colours it (`set` is the
 *  set's `summarizePrSet` variant, worst-wins), so a sibling in trouble is never
 *  hidden behind a calm focused PR; otherwise the badge reads the set's state,
 *  keeping the focused PR's local nuance (conflict markers, draft). */
export function setBadgeCls(focused: PrBadge, set: BadgeVariant): string {
  if (set === "pr-fail") return "failing";
  if (set === "warn" || focused === "conflicts") return PR_META.conflicts.cls;
  if (set === "pr-merged") return PR_META.merged.cls;
  if (set === "pr-closed") return PR_META.closed.cls;
  return PR_META[focused === "draft" ? "draft" : "open"].cls;
}

/** "owner/repo" from a github remote URL (https or ssh form), else null. */
export function repoSlug(remoteUrl: string | null | undefined): string | null {
  const m = remoteUrl?.match(/github\.com[/:]([^/]+\/[^/\s]+?)(?:\.git)?$/);
  return m ? m[1] : null;
}
