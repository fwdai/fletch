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
