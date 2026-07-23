import type { GitState, Mergeable, MergeState, PrState } from "@/api";
import type { IconName } from "@/components/Icon";
import { describeMergeGate } from "./mergeGate";

/** Derived git panel state — computed from live GitState, not stored. */
export type GitPanelState =
  | "clean"
  | "changes"
  | "pushed"
  | "conflicts"
  | "pr-open"
  | "pr-closed"
  | "merged"
  | "loading";

/** Map live git + PR state to the panel state. Uncommitted changes outrank
 *  an open PR — the user's in-flight work is the actionable thing; the PR
 *  (and Merge) stays one click away in the menu and the status chip. */
export function deriveState(git: GitState | null, pr: PrState | null): GitPanelState {
  if (!git) return "loading";
  if (git.files.some((f) => f.kind === "conflicted")) return "conflicts";
  if (pr?.state === "merged") return "merged";
  if (git.files.length > 0) return "changes";
  if (pr?.state === "open") return "pr-open";
  if (pr?.state === "closed") return "pr-closed";
  if (git.ahead > 0) return "pushed";
  return "clean";
}

/** The changes-state delegated commit modes. The user's dropdown pick is
 *  persisted globally (settings table) and becomes the default everywhere
 *  until changed. */
export type GitCommitAction = "agent-commit" | "agent-commit-push" | "agent-commit-pr";

export const COMMIT_ACTIONS: readonly GitCommitAction[] = [
  "agent-commit",
  "agent-commit-push",
  "agent-commit-pr",
];

export function isCommitAction(v: unknown): v is GitCommitAction {
  return (COMMIT_ACTIONS as readonly unknown[]).includes(v);
}

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
  /** PR-open state: GitHub's coarse tri-state mergeability. The only merge
   *  signal when `mergeState` is unavailable; `"unknown"` means not-yet-computed
   *  (renders as "checking…"), only `"conflicting"` claims a real conflict. */
  mergeable?: Mergeable;
  /** PR-open state: GitHub's combined merge gate (spec §6). Null/omitted =
   *  checks unavailable → fall back to `mergeable`-only behavior. */
  mergeState?: MergeState | null;
  /** Number of failing checks (drives copy + the agent-fix CTA). */
  checksFailed?: number;
  /** Changes state: the user's sticky commit mode (defaults to commit & PR). */
  commitAction?: GitCommitAction;
  /** Changes state: a PR is already open for this branch — "open PR" is
   *  meaningless (push updates it) and Merge belongs in the menu. */
  prOpen?: boolean;
  /** Whether GitHub is connected (a valid app token). When false, any
   *  push/PR action is replaced by "Connect GitHub" — local work still runs. */
  githubConnected?: boolean;
  /** Whether the repo has an `origin` remote. When false (a local-only repo)
   *  push/PR give way to "Publish to GitHub". */
  hasOrigin?: boolean;
}

/** Does this commit mode need a GitHub remote (push / open PR)? Plain local
 *  "Commit" does not, so it stays available with no connection. */
function commitModeNeedsRemote(mode: GitCommitAction): boolean {
  return mode === "agent-commit-push" || mode === "agent-commit-pr";
}

/** The action that unblocks GitHub for a repo that can't push yet: connect
 *  first (no token), else publish (no origin remote). `null` when neither is
 *  needed. Shared by the "changes"/"pushed"/"clean" states so the GitHub path
 *  is always one honest, correctly-labelled click away. */
export function githubUnblockAction(counts?: ActionCounts): PrimaryAction | null {
  const { githubConnected = true, hasOrigin = true } = counts ?? {};
  if (!githubConnected) {
    return {
      key: "connect-github",
      label: "Connect GitHub",
      icon: "github",
      statusLabel: "connect GitHub to push & open PRs",
      statusKind: "info",
      tone: "ghost",
    };
  }
  if (!hasOrigin) {
    return {
      key: "publish",
      label: "Publish to GitHub",
      icon: "github",
      statusLabel: "local project — publish to push & open PRs",
      statusKind: "info",
      tone: "ghost",
    };
  }
  return null;
}

/** Maps a git panel state to the panel's primary call-to-action.
 *  Pass counts for dynamic status labels; falls back to generic copy. */
export function primaryFor(state: GitPanelState, counts?: ActionCounts): PrimaryAction {
  const {
    files = 0,
    ahead = 0,
    behind = 0,
    prNumber,
    base = "main",
    customActive = false,
    mergeable = "unknown",
    mergeState = null,
    checksFailed = 0,
    commitAction = "agent-commit-pr",
    prOpen = false,
  } = counts ?? {};
  const prLabel = prNumber != null ? `PR #${prNumber}` : "PR";

  switch (state) {
    case "loading":
      return {
        key: "loading",
        label: "Loading…",
        icon: "refresh",
        statusLabel: "loading git state",
        statusKind: "info",
      };
    case "changes": {
      // Override: user typed their own message → direct commit, agent bypassed.
      if (customActive) {
        return {
          key: "commit-direct",
          label: "Commit",
          icon: "commit",
          statusLabel: "Direct commit",
          statusKind: "ready",
        };
      }
      // Default: delegate to the agent, in the user's sticky commit mode.
      // With a PR already open, "open PR" degrades to "push" — that's what
      // updates the existing PR.
      const effective: GitCommitAction =
        prOpen && commitAction === "agent-commit-pr" ? "agent-commit-push" : commitAction;
      // Offline / local-only: a push/PR mode can't run, so the primary becomes
      // plain local Commit (still fully functional) and the GitHub path is
      // offered in the menu — never a button that fails on click.
      if (commitModeNeedsRemote(effective) && githubUnblockAction(counts)) {
        return {
          key: "agent-commit",
          label: "Commit",
          icon: "commit",
          statusLabel: "Ready to commit",
          statusKind: "warn",
          statusExtra: `${files} ${files === 1 ? "file" : "files"}`,
        };
      }
      const common = {
        statusLabel: "Ready to commit",
        statusKind: "warn" as StatusKind,
        statusExtra: `${files} ${files === 1 ? "file" : "files"}`,
      };
      switch (effective) {
        case "agent-commit":
          return { key: "agent-commit", label: "Commit", icon: "commit", ...common };
        case "agent-commit-push":
          return { key: "agent-commit-push", label: "Commit & push", icon: "push", ...common };
        default:
          return { key: "agent-commit-pr", label: "Commit & open PR", icon: "pr", ...common };
      }
    }
    case "pushed": {
      // Opening a PR needs GitHub — if the token was cleared since the push,
      // offer Connect instead of a PR button that would fail.
      const unblock = githubUnblockAction(counts);
      if (unblock) return unblock;
      // The PR description is the agent's job by default (it has the full
      // context of the branch); the direct gh --fill PR stays in the menu.
      return {
        key: "agent-open-pr",
        label: "Open PR",
        icon: "pr",
        statusLabel:
          ahead === 1 ? "1 commit pushed, no PR yet" : `${ahead} commits pushed, no PR yet`,
        statusKind: "info",
      };
    }
    case "pr-open": {
      // Gate semantics (which MergeState means what, in which tone) live in
      // describeMergeGate; here we only pick the action + status phrasing.
      const gate = describeMergeGate(mergeState, { checksFailed, mergeable });
      const status = (text: string): Pick<PrimaryAction, "statusLabel" | "statusKind"> => ({
        statusLabel: `${prLabel} · ${text}`,
        statusKind: gate.tone,
      });
      const merge = (text: string, tone?: ActionTone): PrimaryAction => ({
        key: "merge",
        label: "Merge PR",
        icon: "merge",
        ...(tone ? { tone } : {}),
        ...status(text),
      });
      switch (gate.situation) {
        case "ready":
          return merge("ready to merge", "success");
        case "mergeable-soft":
          // Only NON-required checks failing — merging is allowed, but say so.
          return merge("optional checks failing");
        case "checks-failing":
          // Failing required checks are agent-fixable.
          return {
            key: "agent-fix",
            label: "Fix checks with agent",
            icon: "wrench",
            ...status(`${checksFailed} ${checksFailed === 1 ? "check" : "checks"} failing`),
          };
        case "review-required":
          // A pure review gate is not agent-fixable — send the user to GitHub.
          return {
            key: "view-pr",
            label: "View on GitHub",
            icon: "github",
            ...status("review required"),
          };
        case "behind":
          return {
            key: "agent-update-branch",
            label: "Update branch",
            icon: "branch",
            ...status(`behind ${base}`),
          };
        case "conflicts":
          return {
            key: "agent-update-branch",
            label: "Update branch",
            icon: "branch",
            ...status(`conflicts with ${base}`),
          };
        case "draft":
          return {
            key: "view-pr",
            label: "View draft on GitHub",
            icon: "github",
            tone: "ghost",
            ...status("draft"),
          };
        case "computing":
          return merge("checking…");
        case "no-conflicts":
          // No checks data — `mergeable` only reports the absence of merge
          // conflicts, NOT CI status, so claim no more than that.
          return merge("no conflicts");
        default:
          // Defensive: a still-resolving gate renders as "checking…", never a
          // false "can't merge" (the old cant-merge overclaim, now removed).
          return merge("checking…");
      }
    }
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
            statusLabel:
              behind === 1 ? `1 commit behind ${base}` : `${behind} commits behind ${base}`,
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
  const {
    behind = 0,
    unpushed = 0,
    base = "main",
    customActive = false,
    mergeable = "unknown",
    mergeState = null,
    checksFailed = 0,
    prOpen = false,
  } = counts ?? {};
  // In local/offline mode, offer the connect-or-publish action in the states
  // where the user would reach for GitHub, so it's always one click away.
  // Publishing offers both visibilities: the primary/first item publishes
  // private (the safe default, matching New Project), with a public variant
  // right beneath it so users don't have to change it on GitHub afterward.
  const unblock = githubUnblockAction(counts);
  const unblockItem: SecondaryAction[] = !unblock
    ? []
    : unblock.key === "publish"
      ? [
          { key: "publish", label: "Publish (private)", icon: "github" },
          { key: "publish-public", label: "Publish (public)", icon: "github" },
        ]
      : [{ key: unblock.key, label: unblock.label, icon: unblock.icon }];
  // "Push more commits" only makes sense when there's something unpushed.
  const pushItem: SecondaryAction[] =
    unpushed > 0 ? [{ key: "push", label: "Push more commits", icon: "push" }] : [];
  switch (state) {
    case "changes":
      // Override active → primary is direct "Commit"; offer a direct
      // "Commit & open PR" (your message + agent-written PR) as the alternate.
      // Offline, the push/PR alternates give way to connect-or-publish.
      if (customActive) {
        return [
          ...(unblock
            ? unblockItem
            : [
                { key: "commit-pr-direct", label: "Commit & open PR", icon: "pr" as IconName },
                { key: "push", label: "Push only", icon: "push" as IconName },
              ]),
          { key: "stash", label: "Stash changes", icon: "inbox" },
          { key: "discard", label: "Discard all", icon: "trash" },
        ];
      }
      // Default (agent): every commit mode is a candidate — the panel drops
      // whichever is the sticky primary. With a PR open, "open PR" is
      // meaningless (push updates it) and Merge joins the menu. Offline, the
      // push/PR items give way to the single connect-or-publish action.
      return [
        { key: "agent-commit", label: "Commit", icon: "commit" },
        ...(unblock
          ? unblockItem
          : [
              { key: "agent-commit-push", label: "Commit & push", icon: "push" as IconName },
              ...(prOpen
                ? [{ key: "merge", label: "Merge PR without changes", icon: "merge" as IconName }]
                : [{ key: "agent-commit-pr", label: "Commit & open PR", icon: "pr" as IconName }]),
              { key: "push", label: "Push only", icon: "push" as IconName },
            ]),
        { key: "stash", label: "Stash changes", icon: "inbox" },
        { key: "discard", label: "Discard all", icon: "trash" },
      ];
    case "pushed":
      // Offline, the PR/push items give way to connect-or-publish; the local
      // Pull stays available either way.
      if (unblock) return [...unblockItem, { key: "pull", label: "Pull", icon: "inbox" }];
      // Primary is the agent-written PR; the direct gh --fill PR stays one
      // click away for users who don't want to wait on the agent.
      return [
        { key: "open-pr", label: "Open PR (auto-fill)", icon: "pr" },
        ...pushItem,
        { key: "pull", label: "Pull", icon: "inbox" },
      ];
    case "pr-open": {
      // Candidates may include the state's primary — the panel filters that
      // out, so every alternate stays reachable regardless of merge_state.
      // "Update branch with agent" (sync base → resolve → push) is distinct
      // from the local-merge "Resolve with agent" used in the conflicts state.
      // "View on GitHub" is deliberately absent: it's a convenience link,
      // rendered as a chip next to the status text, not an action.
      const { needsUpdate } = describeMergeGate(mergeState, { checksFailed, mergeable });
      return [
        { key: "merge", label: "Merge PR", icon: "merge" },
        ...(needsUpdate
          ? [
              {
                key: "agent-update-branch",
                label: "Update branch with agent",
                icon: "branch" as IconName,
              },
            ]
          : []),
        ...(checksFailed > 0
          ? [{ key: "agent-fix", label: "Fix checks with agent", icon: "wrench" as IconName }]
          : []),
        ...pushItem,
        { key: "pull", label: "Pull", icon: "inbox" },
      ];
    }
    case "conflicts":
      return [
        { key: "abort", label: "Abort merge", icon: "close" },
        { key: "view-pr", label: "View on GitHub", icon: "github" },
      ];
    case "pr-closed":
      // Primary is "Open new PR"; the menu offers the alternates only.
      return [...pushItem, { key: "view-pr", label: "View on GitHub", icon: "github" }];
    case "merged":
      return [{ key: "delete-branch", label: "Delete branch", icon: "trash" }];
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
