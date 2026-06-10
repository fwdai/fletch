import type { IconName } from "../Icon";

/** Derived git panel state — computed from live GitState, not stored. */
export type GitPanelState = "clean" | "changes" | "pushed" | "conflicts" | "pr-open" | "pr-closed" | "merged" | "loading";

/** Drives the status-dot color in the action bar. */
export type StatusKind = "clean" | "warn" | "info" | "attention" | "ready" | "merged" | "alert";

/** Button styling for the split action. Default is the accent fill. */
export type ActionTone = "accent" | "ghost" | "success" | "merged" | "danger";

export interface PrimaryAction {
  /** Stable action key — also used as the default selection in the split
   *  button so the primary and the menu share one dispatch table. */
  key: string;
  label: string;
  icon: IconName;
  statusLabel: string;
  statusKind: StatusKind;
  statusExtra?: string;
  /** Visual tone of the CTA. Omitted → accent fill. */
  tone?: ActionTone;
}

export interface SecondaryAction {
  key: string;
  label: string;
  icon: IconName;
  kbd?: string;
}

export interface ActionCounts {
  files?: number;
  ahead?: number;
  behind?: number;
  /** Commits not yet on the upstream; gates the "Push more commits" action. */
  unpushed?: number;
  prNumber?: number;
  base?: string;
  /** Changes state: the user opened the override field and typed a message, so
   *  the commit is direct (agent bypassed) rather than delegated. */
  customActive?: boolean;
  /** PR-open state: whether GitHub reports the PR cleanly mergeable. Gates the
   *  Merge CTA — when false the panel reads as "can't merge yet" (attention). */
  mergeable?: boolean;
}

/** Maps a git panel state to the panel's primary call-to-action.
 *  Pass counts for dynamic status labels; falls back to generic copy. */
export function primaryFor(state: GitPanelState, counts?: ActionCounts): PrimaryAction {
  const { files = 0, ahead = 0, behind = 0, prNumber, base = "main", customActive = false, mergeable = false } = counts ?? {};
  const prLabel = prNumber != null ? `PR #${prNumber}` : "PR";

  switch (state) {
    case "loading":
      return { key: "loading", label: "Loading…", icon: "refresh", statusLabel: "loading git state", statusKind: "info" };
    case "changes":
      // Override: user typed their own message → direct commit, agent bypassed.
      if (customActive) {
        return { key: "commit-direct", label: "Commit", icon: "commit", statusLabel: "Direct commit", statusKind: "ready" };
      }
      // Default: delegate the whole thing to the agent.
      return {
        key: "agent-commit-pr",
        label: "Commit & open PR",
        icon: "pr",
        statusLabel: "Ready to commit",
        statusKind: "warn",
        statusExtra: `${files} ${files === 1 ? "file" : "files"}`,
      };
    case "pushed":
      return {
        key: "open-pr",
        label: "Open PR",
        icon: "pr",
        statusLabel: ahead === 1 ? "1 commit pushed, no PR yet" : `${ahead} commits pushed, no PR yet`,
        statusKind: "info",
      };
    case "pr-open":
      // Merge is the goal here. GitHub's `mergeable` only reports the absence of
      // merge conflicts — NOT CI/check status — so we claim no more than that:
      // "no conflicts", a neutral accent CTA (no green "all clear" signal until
      // real check state lands). When not mergeable the same Merge button reads
      // as an attention state and is disabled by the panel until it clears.
      return mergeable
        ? {
            key: "merge",
            label: "Merge PR",
            icon: "merge",
            statusLabel: `${prLabel} · no conflicts`,
            statusKind: "info",
          }
        : {
            key: "merge",
            label: "Merge PR",
            icon: "merge",
            statusLabel: `${prLabel} · can’t merge yet`,
            statusKind: "attention",
          };
    case "conflicts":
      // Fixable, not fatal — the agent can reconcile the conflict for you.
      return {
        key: "agent-resolve",
        label: "Resolve with agent",
        icon: "merge",
        statusLabel: "merge conflicts",
        statusKind: "attention",
      };
    case "pr-closed":
      return {
        key: "open-pr",
        label: "Open new PR",
        icon: "pr",
        statusLabel: `${prLabel} · closed`,
        statusKind: "info",
      };
    case "merged":
      return {
        key: "archive",
        label: "Archive workspace",
        icon: "archive",
        tone: "merged",
        statusLabel: `${prLabel} · merged`,
        statusKind: "merged",
      };
    default: // clean
      // Working tree is clean. The useful move depends on the base branch:
      // if it has advanced (behind > 0), rebasing catches up; otherwise a
      // pull syncs the branch with its upstream. Quiet — a ghost button.
      return behind > 0
        ? {
            key: "rebase",
            label: `Rebase onto ${base}`,
            icon: "branch",
            tone: "ghost",
            statusLabel: behind === 1 ? `1 commit behind ${base}` : `${behind} commits behind ${base}`,
            statusKind: "info",
          }
        : {
            key: "pull",
            label: "Pull",
            icon: "inbox",
            tone: "ghost",
            statusLabel: "working tree clean",
            statusKind: "clean",
          };
  }
}

export function secondaryFor(state: GitPanelState, counts?: ActionCounts): SecondaryAction[] {
  const { behind = 0, unpushed = 0, base = "main", customActive = false, mergeable = false } = counts ?? {};
  // "Push more commits" only makes sense when there's something unpushed.
  const pushItem: SecondaryAction[] =
    unpushed > 0 ? [{ key: "push", label: "Push more commits", icon: "push" }] : [];
  switch (state) {
    case "changes":
      // Override active → primary is direct "Commit"; offer a direct
      // "Commit & open PR" (your message + agent-written PR) as the alternate.
      if (customActive) {
        return [
          { key: "commit-pr-direct", label: "Commit & open PR", icon: "pr" },
          { key: "push", label: "Push only", icon: "push" },
          { key: "stash", label: "Stash changes", icon: "inbox" },
          { key: "discard", label: "Discard all", icon: "trash" },
        ];
      }
      // Default (agent) → primary is delegated "Commit & open PR"; offer a
      // delegated "Commit only" as the alternate.
      return [
        { key: "agent-commit", label: "Commit only", icon: "commit", kbd: "⌘K" },
        { key: "push", label: "Push only", icon: "push" },
        { key: "stash", label: "Stash changes", icon: "inbox" },
        { key: "discard", label: "Discard all", icon: "trash" },
      ];
    case "pushed":
      // Primary is "Open PR"; the menu offers the alternates only.
      return [
        ...pushItem,
        { key: "pull", label: "Pull", icon: "inbox" },
      ];
    case "pr-open":
      // Primary is "Merge PR". Surface the PR link, and — when it can't merge
      // yet (base advanced / conflicts with base) — an agent-delegated branch
      // update (sync base → resolve → push), distinct from the local-merge
      // "Resolve with agent" used in the conflicts state.
      return [
        { key: "view-pr", label: "View on GitHub", icon: "github" },
        ...(mergeable ? [] : [{ key: "agent-update-branch", label: "Update branch with agent", icon: "branch" as IconName }]),
        ...pushItem,
        { key: "pull", label: "Pull", icon: "inbox" },
      ];
    case "conflicts":
      return [
        { key: "abort", label: "Abort merge", icon: "close" },
        { key: "view-pr", label: "View on GitHub", icon: "github" },
      ];
    case "pr-closed":
      // Primary is "Open new PR"; the menu offers the alternates only.
      return [
        ...pushItem,
        { key: "view-pr", label: "View on GitHub", icon: "github" },
      ];
    case "merged":
      return [
        { key: "delete-branch", label: "Delete branch", icon: "trash" },
      ];
    case "clean":
      // Mirror of the clean primary: offer the action the primary didn't take,
      // so both Pull and Rebase-onto-base are always reachable.
      return behind > 0
        ? [{ key: "pull", label: "Pull", icon: "inbox" }]
        : [{ key: "rebase", label: `Rebase onto ${base}`, icon: "branch" }];
    default:
      return [];
  }
}
