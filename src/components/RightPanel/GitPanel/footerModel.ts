// ── The footer: the ladder's next step as a button, and everything else as a
// menu ────────────────────────────────────────────────────────────────────────
// `readiness.ts` decides what to do next (`nextRung`); this module says it as
// the split button's primary action, so the footer can never point somewhere
// the ladder's current row doesn't. The menu is the rest of what the checkout
// can do, in groups: the commit mode (a sticky default, marked), the branch,
// the pull request, GitHub when it is not connected, and the destructive
// working-tree actions last, behind a second click.
//
// Pure, like `ladderModel.ts`: every state has a test, and no store or React.

import type { IconName } from "@/components/Icon";
import type { ActionTone, GitCommitAction } from "@/components/RightPanel/primaryActions";
import { describeMergeGate } from "@/mergeGate";
import { nextRung, type Rung } from "@/readiness";
import { plural } from "@/util/format";
import type { LadderInput } from "./ladderModel";

export interface FooterAction {
  /** The key `runAction` dispatches — or a commit mode, which the panel makes
   *  the sticky default instead of running. */
  key: string;
  label: string;
  icon: IconName;
  /** Visual tone of the primary. Omitted → the accent fill. */
  tone?: ActionTone;
  /** Dimmed and inert: offered so the user knows it exists, but not now. */
  disabled?: boolean;
  /** Why it is disabled, when that is worth a tooltip (`applyGate`). */
  reason?: string;
  /** The current pick in a radio group. */
  active?: boolean;
  danger?: boolean;
  /** A destructive action's second-click label: the first click arms it. */
  confirm?: string;
}

export interface FooterGroup {
  /** Section heading; none for the trailing destructive group. */
  title?: string;
  /** Picking an item selects it (the commit mode) rather than running it. */
  radio?: boolean;
  items: FooterAction[];
}

export interface Footer {
  primary: FooterAction;
  groups: FooterGroup[];
}

/** The footer of a lane the panel can only look at — another PR of the
 *  checkout than the focused one. The host's PR actions and autopilot work on
 *  the focused PR, so acting on this one means focusing it first; that is the
 *  one place the panel moves the focus, and it says so. */
export function focusFooter(): Footer {
  return {
    primary: { key: "focus-pr", label: "Focus this PR", icon: "pr", tone: "ghost" },
    groups: [{ items: [{ key: "view-pr", label: "View on GitHub", icon: "github" }] }],
  };
}

/** The footer with one gate run over every action it offers: a closed gate
 *  disables the row and carries its reason; the primary's reason comes back
 *  as `mainReason`, or null when the gate asked for quiet. Radio rows (the
 *  commit mode) are a choice, not a dispatch, so they are never gated. */
export function applyGate(
  footer: Footer,
  gate: (key: string) => { reason: string; quiet?: boolean } | null,
): Footer & { mainDisabled: boolean; mainReason: string | null } {
  const gated = (a: FooterAction): FooterAction => {
    const g = gate(a.key);
    return g ? { ...a, disabled: true, reason: g.reason } : a;
  };
  const main = gate(footer.primary.key);
  return {
    primary: gated(footer.primary),
    groups: footer.groups.map((g) => (g.radio ? g : { ...g, items: g.items.map(gated) })),
    mainDisabled: main != null,
    mainReason: main && !main.quiet ? main.reason : null,
  };
}

export interface FooterInput extends LadderInput {
  /** The user's sticky commit mode. */
  commitMode: GitCommitAction;
  /** The user opened the message field and typed: the commit is direct, the
   *  agent bypassed. */
  customActive: boolean;
  githubConnected: boolean;
  hasOrigin: boolean;
}

/** The commit modes as the footer offers them, in the order of how much they
 *  do. The ladder's `commitMode` is the same triple without the prefix. */
const COMMIT_MODES: readonly { key: GitCommitAction; label: string; icon: IconName }[] = [
  { key: "agent-commit", label: "Commit", icon: "commit" },
  { key: "agent-commit-push", label: "Commit & push", icon: "push" },
  { key: "agent-commit-pr", label: "Commit & open PR", icon: "pr" },
];

export function ladderCommitMode(mode: GitCommitAction): "commit" | "commit-push" | "commit-pr" {
  return mode === "agent-commit-pr"
    ? "commit-pr"
    : mode === "agent-commit-push"
      ? "commit-push"
      : "commit";
}

/** The action that unblocks GitHub for a repo that can't push yet: connect
 *  first (no token), else publish (no origin remote). Null when neither is
 *  needed. Any primary that would push or open a PR gives way to this, so the
 *  GitHub path is always one honest, correctly-labelled click away. */
export function githubUnblock(
  input: Pick<FooterInput, "githubConnected" | "hasOrigin">,
): FooterAction | null {
  if (!input.githubConnected) {
    return { key: "connect-github", label: "Connect GitHub", icon: "github", tone: "ghost" };
  }
  if (!input.hasOrigin) {
    return { key: "publish", label: "Publish to GitHub", icon: "github", tone: "ghost" };
  }
  return null;
}

const VIEW_PR: FooterAction = { key: "view-pr", label: "View on GitHub", icon: "github" };

/** The primary for a rung. `unblock` replaces anything that would reach GitHub. */
function primaryOf(rung: Rung, input: FooterInput, unblock: FooterAction | null): FooterAction {
  const { git, pr, customActive, base } = input;
  const unpushed = git?.unpushed ?? 0;
  switch (rung.do) {
    case "wait":
      return rung.why === "unknown-state"
        ? { key: "loading", label: "Loading…", icon: "refresh" }
        : // The merge gate is still being read: nothing to decide yet.
          { ...VIEW_PR, tone: "ghost" };
    case "delegate":
      switch (rung.kind) {
        case "resolve":
          return { key: "agent-resolve", label: "Resolve with agent", icon: "merge" };
        case "commit":
        case "commit-push":
        case "commit-pr": {
          // The user's own message commits directly, the agent bypassed.
          if (customActive) return { key: "commit-direct", label: "Commit", icon: "commit" };
          // Offline / local-only: a push or PR mode can't run, so the primary
          // is the plain local commit (still fully functional) and the GitHub
          // path sits in the menu — never a button that fails on click.
          if (rung.kind !== "commit" && unblock) return COMMIT_MODES[0];
          const mode = COMMIT_MODES.find((m) => ladderCommitMode(m.key) === rung.kind);
          return mode ?? COMMIT_MODES[0];
        }
        case "open-pr":
          return (
            unblock ?? {
              key: "agent-open-pr",
              label: pr?.state === "merged" ? "Open follow-up PR" : "Open PR",
              icon: "pr",
            }
          );
        case "push":
          return (
            unblock ?? { key: "push", label: `Push ${plural(unpushed, "commit")}`, icon: "push" }
          );
        case "update-branch":
          return { key: "agent-update-branch", label: "Update branch", icon: "branch" };
        case "fix-checks":
          return { key: "agent-fix", label: "Fix checks with agent", icon: "wrench" };
        case "resolve-comments":
          return {
            key: "agent-resolve-comments",
            label: "Resolve comments with agent",
            icon: "chatPlus",
          };
      }
      break;
    case "escalate":
      switch (rung.blocker.kind) {
        case "proposal-closed":
          return unblock ?? { key: "agent-open-pr", label: "Open new PR", icon: "pr" };
        case "draft":
          return { ...VIEW_PR, label: "View draft on GitHub", tone: "ghost" };
        default:
          // A review gate, or threads we already answered: a person's move.
          return VIEW_PR;
      }
    case "merge":
      return unblock ?? { key: "merge", label: "Merge PR", icon: "merge", tone: "success" };
    case "ready":
      // Nothing blocking. With an open PR the gate just isn't positively open
      // (no checks data), so the decision is GitHub's to show; without one the
      // tree is clean and the quiet, useful move is to sync.
      if (pr?.state === "open") return { ...VIEW_PR, tone: "ghost" };
      return (git?.behind ?? 0) > 0
        ? { key: "rebase", label: `Rebase onto ${base}`, icon: "branch", tone: "ghost" }
        : { key: "pull", label: "Pull", icon: "inbox", tone: "ghost" };
    case "landed":
      // Shipped. Archiving and deleting the branch sit on the merged PR's row;
      // the footer keeps the quiet sync, since the next thing is a follow-up.
      return { key: "pull", label: "Pull", icon: "inbox", tone: "ghost" };
  }
  return { key: "loading", label: "Loading…", icon: "refresh" };
}

export function buildFooter(input: FooterInput): Footer {
  const { git, pr, checks, comments, mergeState, base, commitMode, customActive } = input;
  const rung: Rung = git
    ? nextRung({ git, pr, checks, comments }, { base, commitMode: ladderCommitMode(commitMode) })
    : { do: "wait", why: "unknown-state" };
  const unblock = githubUnblock(input);
  const primary = primaryOf(rung, input, unblock);
  // Nothing is known yet: no menu either, rather than a Pull that would run
  // against a checkout the panel hasn't read.
  if (!git) return { primary, groups: [] };

  const files = git.files.length;
  const conflicted = git.files.some((f) => f.kind === "conflicted");
  const { unpushed, ahead } = git;
  const prOpen = pr?.state === "open";
  const gate = prOpen
    ? describeMergeGate(mergeState, {
        checksFailed: checks?.required_failing.length ?? 0,
        mergeable: pr.mergeable,
      })
    : null;
  const threadsForUs = (comments?.unresolved ?? []).filter((t) => !t.we_replied_last).length;
  // The next step is a commit: the mode group belongs in the menu.
  const committing =
    rung.do === "delegate" &&
    (rung.kind === "commit" || rung.kind === "commit-push" || rung.kind === "commit-pr");

  const groups: FooterGroup[] = [];

  // ── Commit mode: a sticky default, not an action ───────────────────────────
  if (committing && !customActive && !unblock) {
    groups.push({
      title: "Commit mode",
      radio: true,
      // With a PR open, "open PR" is meaningless — a push updates it.
      items: COMMIT_MODES.filter((m) => !(prOpen && m.key === "agent-commit-pr")).map((m) => ({
        ...m,
        active:
          m.key === commitMode ||
          (prOpen && commitMode === "agent-commit-pr" && m.key === "agent-commit-push"),
      })),
    });
  } else if (committing && customActive && !unblock && !prOpen) {
    // Your message, the agent's PR description.
    groups.push({
      title: "Commit",
      items: [{ key: "commit-pr-direct", label: "Commit & open PR", icon: "pr" }],
    });
  }

  // ── Branch ─────────────────────────────────────────────────────────────────
  groups.push({
    title: "Branch",
    items: [
      ...(unpushed > 0 && !unblock
        ? [{ key: "push", label: `Push ${plural(unpushed, "commit")}`, icon: "push" as IconName }]
        : []),
      { key: "pull", label: "Pull", icon: "inbox" },
      { key: "rebase", label: `Rebase onto ${base}`, icon: "branch" },
    ],
  });

  // ── Pull request ───────────────────────────────────────────────────────────
  if (!unblock) {
    const items: FooterAction[] = [];
    if (prOpen && gate) {
      items.push({ key: "merge", label: "Merge PR", icon: "merge", disabled: !gate.mergeAllowed });
      if (gate.needsUpdate) {
        items.push({
          key: "agent-update-branch",
          label: "Update branch with agent",
          icon: "branch",
        });
      }
      if ((checks?.required_failing.length ?? 0) > 0) {
        items.push({ key: "agent-fix", label: "Fix checks with agent", icon: "wrench" });
      }
      if (threadsForUs > 0) {
        items.push({
          key: "agent-resolve-comments",
          label: "Resolve comments with agent",
          icon: "chatPlus",
        });
      }
    } else if (ahead > 0 && files === 0) {
      // Pushed, no open PR: the agent-written PR is the primary; the direct
      // auto-fill one is the alternative for anyone not waiting on the agent.
      items.push({ key: "open-pr", label: "Open PR (auto-fill)", icon: "pr" });
    }
    if (items.length > 0) groups.push({ title: "Pull request", items });
  }

  // ── GitHub, when the repo can't reach it yet ───────────────────────────────
  if (unblock) {
    groups.push({
      title: "GitHub",
      items:
        unblock.key === "publish"
          ? [
              { key: "publish", label: "Publish (private)", icon: "github" },
              { key: "publish-public", label: "Publish (public)", icon: "github" },
            ]
          : [{ key: unblock.key, label: unblock.label, icon: unblock.icon }],
    });
  }

  // ── Working tree: the destructive moves, last and behind a second click ────
  if (files > 0) {
    groups.push({
      items: [
        ...(conflicted
          ? [
              {
                key: "abort",
                label: "Abort merge",
                icon: "close" as IconName,
                danger: true,
                confirm: "Abort the merge? Click again",
              },
            ]
          : [{ key: "stash", label: "Stash changes", icon: "inbox" as IconName }]),
        {
          key: "discard",
          label: "Discard all changes",
          icon: "trash",
          danger: true,
          confirm: `Discard ${plural(files, "file")}? Click again`,
        },
      ],
    });
  }

  // The primary is never also a menu row, and a group that only held it is
  // gone. The radio group is the exception: its rows are the choices, the
  // current one marked — including the one the primary currently is.
  const filtered = groups
    .map((g) => (g.radio ? g : { ...g, items: g.items.filter((i) => i.key !== primary.key) }))
    .filter((g) => g.items.length > 0);
  return { primary, groups: filtered };
}
