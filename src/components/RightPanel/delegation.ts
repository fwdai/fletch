import type { GitState, PrChecks, PrState } from "../../api";

/** One agent-delegated git action: the user clicked a panel action whose
 *  judgment part (message, description, conflict edits) belongs to the
 *  coding agent; the agent executes the mutation through the app's file
 *  RPC (`git_commit` / `open_pr` / `git_update_branch` / `git_push`). */
export type GitDelegationKind =
  | "commit"
  | "commit-pr"
  | "open-pr"
  | "resolve"
  | "update-branch"
  | "fix-checks";

export interface GitDelegation {
  kind: GitDelegationKind;
  /** Epoch ms when the delegation was sent — grace window for status races. */
  startedAt: number;
  /** The agent has been observed `running` since `startedAt`. Until then an
   *  `idle` status is the pre-send value, not a finished turn. */
  sawRunning: boolean;
}

/** Footer status line while the agent holds control. */
export function delegationLabel(kind: GitDelegationKind): string {
  switch (kind) {
    case "commit":        return "Agent is writing the commit message…";
    case "commit-pr":     return "Agent is committing & opening a PR…";
    case "open-pr":       return "Agent is writing the PR description…";
    case "resolve":       return "Agent is resolving the conflicts…";
    case "update-branch": return "Agent is updating the branch…";
    case "fix-checks":    return "Agent is fixing the failing checks…";
  }
}

/** Success notice once the watched transition lands. */
export function delegationDone(kind: GitDelegationKind): string {
  switch (kind) {
    case "commit":        return "Agent committed your changes";
    case "commit-pr":     return "Committed — PR is open";
    case "open-pr":       return "PR is open";
    case "resolve":       return "Conflicts resolved";
    case "update-branch": return "Branch updated";
    case "fix-checks":    return "Agent finished — checks are re-running";
  }
}

/** Whether the git/PR transition this delegation is waiting for has landed.
 *  Pure — the panel evaluates it against each poll tick. `fix-checks` is the
 *  exception: CI re-runs take minutes, so the caller resolves it as soon as
 *  the agent goes idle and lets the checks polling carry the story from there. */
export function delegationResolved(
  kind: GitDelegationKind,
  git: GitState | null,
  pr: PrState | null,
  checks: PrChecks | null,
): boolean {
  switch (kind) {
    case "commit":
      return git != null && git.files.length === 0;
    case "commit-pr":
    case "open-pr":
      return pr?.state === "open";
    case "resolve":
      return git != null && !git.files.some((f) => f.kind === "conflicted");
    case "update-branch":
      // `unknown` = GitHub still recomputing after a push — keep waiting.
      if (checks) return !["behind", "dirty", "unknown"].includes(checks.merge_state);
      return pr?.mergeable === true;
    case "fix-checks":
      return false;
  }
}
