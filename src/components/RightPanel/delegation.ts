import type { AgentStatus, GitState, PrChecks, PrState } from "@/api";

/** One agent-delegated git action: the user clicked a panel action whose
 *  judgment part (message, description, conflict edits) belongs to the
 *  coding agent. The agent runs local mutations (commit, merge, conflict
 *  resolution) as plain in-sandbox git, and the credentialed remote actions
 *  through the app's file RPC (`open_pr` / `git_push` / `git_fetch`). The
 *  `agent:git-action` signal that confirms a local mutation arrives via the
 *  clone's `post-commit` / `post-merge` hooks (see `gitActionProvesKind`). */
export type GitDelegationKind =
  | "commit"
  | "commit-push"
  | "commit-pr"
  | "open-pr"
  | "push"
  | "resolve"
  | "update-branch"
  | "fix-checks";

export interface GitDelegation {
  kind: GitDelegationKind;
  /** The `[app-action]` trigger to deliver to the agent. Held here (not sent
   *  immediately) when the delegation is `queued`, so it can be delivered once
   *  the agent is idle — see `queued`. */
  prompt: string;
  /** Epoch ms when the delegation entered the current phase: set at send,
   *  reset on dequeue. The give-up grace window counts from here. */
  startedAt: number;
  /** OUR turn has been observed `running` since `startedAt`. Until then a
   *  settled status is pre-send state, not a finished delegation turn. Used
   *  only to arm the give-up clock — never to confirm success. */
  sawRunning: boolean;
  /** The agent ran a successful git op matching THIS delegation's kind during
   *  our turn — the backend's ground-truth `agent:git-action` signal, filtered
   *  by `gitActionProvesKind`. This is the causal link a snapshot can't provide:
   *  it distinguishes a target the agent reached from one already satisfied by a
   *  manual action or pre-existing state. Ignored while `queued` (those ops
   *  belong to the turn we're waiting behind), which is sound because we don't
   *  deliver our trigger until that turn ends — so our own turn runs in
   *  isolation and its ops can't be confused with the prior turn's. */
  sawGitOp: boolean;
  /** The agent was already running when the action was clicked, so our trigger
   *  is held undelivered (`prompt`) rather than injected mid-turn — a mid-turn
   *  injection would fold into the running turn (Claude coalesces stdin into the
   *  current turn) instead of running as its own. We wait for the agent to go
   *  idle, then deliver and drop `queued` (the delegated turn now runs alone). */
  queued: boolean;
}

/** How long a settled agent may sit without `sawRunning` before the
 *  delegation reads as abandoned. Covers send→turn-start latency (and the
 *  idle gap between a dequeued trigger and its turn actually starting). */
export const DELEGATION_GIVE_UP_GRACE_MS = 15_000;

/** What the lifecycle watcher should do for the current observation. Pure —
 *  the panel effect maps each step to a store action:
 *  - "resolve": the watched git/PR transition landed → clear + success notice
 *  - "wait": nothing to do this pass
 *  - "dequeue": the pre-existing turn settled → deliver the held trigger, drop
 *    `queued`, reset the clock
 *  - "mark-running": our turn started → set `sawRunning` (arms the give-up clock)
 *  - "give-up": agent settled without the transition → clear + honest notice */
export type DelegationStep = "resolve" | "wait" | "dequeue" | "mark-running" | "give-up";

export function delegationStep(
  delegation: GitDelegation,
  status: AgentStatus,
  resolved: boolean,
  now: number,
): DelegationStep {
  // Resolve only when the world reached the target (`resolved`) AND the agent
  // ran a matching git mutation during OUR turn (`sawGitOp`). Snapshot state
  // alone can't attribute causality: a target already satisfied by a manual
  // stash/discard or a pre-existing clean/open PR would otherwise read as
  // success the agent never produced. `!queued` is belt-and-suspenders — our
  // trigger isn't delivered until the prior turn ends, so `sawGitOp` is never
  // set while queued, but never resolve a still-queued delegation regardless.
  if (resolved && delegation.sawGitOp && !delegation.queued) return "resolve";
  const active = status === "running" || status === "spawning";
  // Queued behind a foreign turn: its activity is not ours to interpret.
  if (delegation.queued) return active ? "wait" : "dequeue";
  if (status === "running" && !delegation.sawRunning) return "mark-running";
  const armed = delegation.sawRunning || now - delegation.startedAt > DELEGATION_GIVE_UP_GRACE_MS;
  if (!active && armed) return "give-up";
  return "wait";
}

/** Does a successful `agent:git-action` op stand as proof that THIS delegation's
 *  requested work ran? The backend emits the event for any successful mutating
 *  op, but a turn we're queued behind can emit an unrelated mutation (e.g. a
 *  `git_push` while we're waiting on a `commit`). Accepting that would let a
 *  pre-satisfied target resolve before the requested action runs, so the op must
 *  belong to the delegation's own playbook. Resolution still ANDs this with the
 *  target snapshot, so listing every op a kind touches (not just the final one)
 *  is safe — the snapshot gates the actual completion. */
export function gitActionProvesKind(kind: GitDelegationKind, op: string): boolean {
  switch (kind) {
    case "commit":
    case "resolve":
      return op === "git_commit";
    case "commit-push":
    case "fix-checks":
      return op === "git_commit" || op === "git_push";
    case "commit-pr":
      return op === "git_commit" || op === "open_pr";
    case "open-pr":
      return op === "open_pr";
    case "push":
      return op === "git_push";
    case "update-branch":
      // A clean merge fires the clone's post-merge hook (`git_update_branch`);
      // a conflicted merge is completed by a native `git commit`, firing
      // post-commit (`git_commit`). Accept either — resolution still ANDs this
      // with the merge-state snapshot, so listing both ops is safe.
      return op === "git_update_branch" || op === "git_commit";
  }
}

/** Marker prefix for app-sent action triggers. The full per-action playbooks
 *  live in the agent's injected instructions (`instructions/git_actions.md`),
 *  so the chat carries only this one-liner — which the transcript folds into
 *  a compact chip instead of a user bubble. */
export const APP_ACTION_PREFIX = "[app-action] ";

/** Build the one-line trigger the app sends when a git action is clicked:
 *  `[app-action] <name> key="value" …`. Params carry only the dynamic context
 *  the static playbook can't know (base branch, failing check names); empty
 *  values are dropped. */
export function appActionMessage(name: string, params?: Record<string, string>): string {
  const parts = [`${APP_ACTION_PREFIX}${name}`];
  for (const [key, value] of Object.entries(params ?? {})) {
    if (!value) continue;
    parts.push(`${key}="${value.replaceAll('"', '\\"')}"`);
  }
  return parts.join(" ");
}

/** Footer status line while the agent holds control. */
export function delegationLabel(kind: GitDelegationKind): string {
  switch (kind) {
    case "commit":
      return "Agent is writing the commit message…";
    case "commit-push":
      return "Agent is committing & pushing…";
    case "commit-pr":
      return "Agent is committing & opening a PR…";
    case "open-pr":
      return "Agent is writing the PR description…";
    case "push":
      return "Agent is naming the branch & pushing…";
    case "resolve":
      return "Agent is resolving the conflicts…";
    case "update-branch":
      return "Agent is updating the branch…";
    case "fix-checks":
      return "Agent is fixing the failing checks…";
  }
}

/** Success notice once the watched transition lands. */
export function delegationDone(kind: GitDelegationKind): string {
  switch (kind) {
    case "commit":
      return "Agent committed your changes";
    case "commit-push":
      return "Committed & pushed";
    case "commit-pr":
      return "Committed — PR is open";
    case "open-pr":
      return "PR is open";
    case "push":
      return "Pushed to origin";
    case "resolve":
      return "Conflicts resolved";
    case "update-branch":
      return "Branch updated";
    case "fix-checks":
      return "Agent finished — checks are re-running";
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
    case "commit-push":
      return git != null && git.files.length === 0 && git.unpushed === 0;
    case "commit-pr":
      // The agent both commits AND opens/updates the PR. A PR may already be
      // open (new changes pushed onto an existing PR's branch), so "PR open"
      // alone is not evidence the action ran — require the working tree to be
      // clean too, proving the commit actually landed this turn.
      return git != null && git.files.length === 0 && pr?.state === "open";
    case "open-pr":
      return pr?.state === "open";
    case "push":
      // Branch materialized and everything's on origin.
      return git != null && git.unpushed === 0;
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
