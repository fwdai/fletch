import type { IconName } from "../Icon";

/** Derived git panel state — computed from live GitState, not stored. */
export type GitPanelState = "clean" | "changes" | "pushed" | "conflicts" | "pr-open" | "merged" | "loading";

export interface PrimaryAction {
  label: string;
  icon: IconName;
  statusLabel: string;
  statusKind: "warn" | "ready" | "alert";
  statusExtra?: string;
  /** Render as a destructive outline button instead of the accent fill. */
  danger?: boolean;
}

export interface SecondaryAction {
  key: string;
  label: string;
  icon: IconName;
  kbd?: string;
}

/** Maps a git panel state to the panel's primary call-to-action. */
export function primaryFor(state: GitPanelState): PrimaryAction {
  switch (state) {
    case "loading":
      return { label: "Loading…", icon: "refresh", statusLabel: "loading git state", statusKind: "ready" };
    case "changes":
      return { label: "Commit & open PR", icon: "pr", statusLabel: "changes uncommitted", statusKind: "warn" };
    case "pushed":
      return { label: "Open PR", icon: "pr", statusLabel: "commit pushed, no PR yet", statusKind: "warn" };
    case "pr-open":
      return { label: "View PR ↗", icon: "external", statusLabel: "PR open", statusKind: "ready", statusExtra: "checks passing" };
    case "conflicts":
      return { label: "Resolve conflicts", icon: "merge", statusLabel: "merge conflicts", statusKind: "alert", danger: true };
    case "merged":
      return { label: "Archive workspace", icon: "check", statusLabel: "PR merged", statusKind: "ready" };
    default:
      return { label: "Nothing to do", icon: "check", statusLabel: "working tree clean", statusKind: "ready" };
  }
}

export function secondaryFor(state: GitPanelState): SecondaryAction[] {
  switch (state) {
    case "changes":
      return [
        { key: "commit", label: "Commit only", icon: "commit", kbd: "⌘K" },
        { key: "push", label: "Push only", icon: "push" },
        { key: "stash", label: "Stash changes", icon: "inbox" },
        { key: "discard", label: "Discard all", icon: "trash" },
      ];
    case "pushed":
      return [
        { key: "open-pr", label: "Open draft PR", icon: "pr" },
        { key: "push", label: "Push more commits", icon: "push" },
        { key: "pull", label: "Pull", icon: "inbox" },
      ];
    case "pr-open":
      return [
        { key: "push", label: "Push more commits", icon: "push" },
        { key: "pull", label: "Pull", icon: "inbox" },
        { key: "merge", label: "Merge PR", icon: "merge" },
        { key: "view-pr", label: "View on GitHub", icon: "github" },
        { key: "request-review", label: "Request review", icon: "user" },
      ];
    case "conflicts":
      return [
        { key: "abort", label: "Abort merge", icon: "close" },
        { key: "view-pr", label: "View on GitHub", icon: "github" },
      ];
    case "merged":
      return [
        { key: "archive", label: "Archive workspace", icon: "check" },
        { key: "delete-branch", label: "Delete branch", icon: "trash" },
        { key: "start-new", label: "Start new agent", icon: "plus" },
      ];
    case "clean":
      return [
        { key: "push", label: "Push", icon: "push" },
        { key: "pull", label: "Pull", icon: "inbox" },
      ];
    default:
      return [];
  }
}
