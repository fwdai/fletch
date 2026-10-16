import type { GitState, PrState } from "@/api";
import { hasWorkSinceMerge } from "@/readiness";

/** Derived git panel state — computed from live GitState, not stored. The
 *  panel's body and footer no longer switch on it (the ladder and footer
 *  models read `readiness.ts` directly); it remains the one-word summary the
 *  commit draft and the composer's visibility key off. */
export type GitPanelState =
  | "clean"
  | "changes"
  | "pushed"
  | "conflicts"
  | "pr-open"
  | "pr-closed"
  | "merged"
  | "loading";

/** Map live git + PR state to the panel state. Uncommitted changes outrank an
 *  open PR — the user's in-flight work is the actionable thing.
 *
 *  A merged PR is not terminal for the workspace, only for that PR: the agent
 *  goes on working in the same worktree and opens follow-up PRs. So `merged`
 *  claims the state only while nothing has happened since the merge. */
export function deriveState(git: GitState | null, pr: PrState | null): GitPanelState {
  if (!git) return "loading";
  if (git.files.some((f) => f.kind === "conflicted")) return "conflicts";
  if (pr?.state === "merged" && !hasWorkSinceMerge(git)) return "merged";
  if (git.files.length > 0) return "changes";
  if (pr?.state === "open") return "pr-open";
  if (pr?.state === "closed") return "pr-closed";
  if (git.ahead > 0) return "pushed";
  return "clean";
}

/** The delegated commit modes. The user's pick is persisted globally (settings
 *  table) and becomes the default everywhere until changed. */
export type GitCommitAction = "agent-commit" | "agent-commit-push" | "agent-commit-pr";

export const COMMIT_ACTIONS: readonly GitCommitAction[] = [
  "agent-commit",
  "agent-commit-push",
  "agent-commit-pr",
];

export function isCommitAction(v: unknown): v is GitCommitAction {
  return (COMMIT_ACTIONS as readonly unknown[]).includes(v);
}

/** Button styling for the split action. Default is the accent fill. */
export type ActionTone = "accent" | "ghost" | "success" | "merged" | "danger";
