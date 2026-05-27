import type { IconName } from "../Icon";

/** Derived git panel state — computed from live GitState, not stored. */
export type GitPanelState = "clean" | "changes" | "pushed" | "conflicts" | "pr-open" | "merged";

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
  label: string;
  icon: IconName;
  kbd?: string;
}

/** Maps a git panel state to the panel's primary call-to-action. */
export function primaryFor(state: GitPanelState): PrimaryAction {
  switch (state) {
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
        { label: "Commit only", icon: "commit", kbd: "⌘K" },
        { label: "Push only", icon: "push" },
        { label: "Stash changes", icon: "inbox" },
        { label: "Discard all", icon: "trash" },
      ];
    case "pushed":
      return [
        { label: "Open draft PR", icon: "pr" },
        { label: "Push more commits", icon: "push" },
      ];
    case "pr-open":
      return [
        { label: "Push more commits", icon: "push" },
        { label: "Request review", icon: "user" },
        { label: "View on GitHub", icon: "github" },
      ];
    case "conflicts":
      return [
        { label: "Abort merge", icon: "close" },
        { label: "View on GitHub", icon: "github" },
      ];
    case "merged":
      return [
        { label: "Delete branch", icon: "trash" },
        { label: "Start new agent", icon: "plus" },
      ];
    default:
      return [];
  }
}
