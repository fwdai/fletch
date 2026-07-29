import type { AgentStatus, GitState, PrChecks, PrState } from "@/api";

// ── Delegation: handing one unit of work to the coding agent ───────────────
// A delegation is "the agent takes it from here" — the judgment part of an
// action (a commit message, a PR description, conflict edits, a test fix)
// belongs to the agent, and the app watches for the transition that proves it
// landed. The mechanism is deliberately NOT git-specific: what varies per kind
// is how the work is detected and remediated, not the lifecycle around it.
//
// Today every kind happens to be a git/GitHub action, and the `*_git_*` op
// names below are honestly git-shaped. What is *not* git-shaped — the
// dispatch/turn/verdict lifecycle, the causality proof, the give-up clock — is
// named for the problem instead, so a future non-git remediation slots in
// without renaming the machinery around it.

/** One unit of work handed to the coding agent. The agent runs local mutations
 *  (commit, merge, conflict resolution) as plain in-sandbox git, and the
 *  credentialed remote actions through the app's file RPC (`open_pr` /
 *  `git_push` / `git_fetch`). The `agent:git-action` signal that confirms a
 *  local mutation arrives via the clone's `post-commit` / `post-merge` hooks
 *  (see `actionProvesKind`). */
export type DelegationKind =
  | "commit"
  | "commit-push"
  | "commit-pr"
  | "open-pr"
  | "push"
  | "resolve"
  | "update-branch"
  | "fix-checks"
  | "resolve-comments";

export interface Delegation {
  kind: DelegationKind;
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
   *  by `actionProvesKind`. This is the causal link a snapshot can't provide:
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
  /** Which checkout of a multi-repo agent the delegation targets: a
   *  `TrackedRepo.subdir` for a secondary repo, undefined for the primary.
   *  Delegations are stored under `checkoutKey(agentId, subdir)`, so this is
   *  the same scope the store key encodes — kept on the record so the trigger
   *  message can carry it as `repo="…"` and the agent works in that sibling
   *  checkout. The lifecycle is evaluated against THAT checkout's git/PR
   *  state (see `delegationResolved`). */
  subdir?: string;
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
  delegation: Delegation,
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
export function actionProvesKind(kind: DelegationKind, op: string): boolean {
  switch (kind) {
    case "commit":
      return op === "git_commit";
    case "resolve":
      // Completing a conflicted merge yields a merge commit, which post-commit
      // reports as `git_update_branch`; a rebase/cherry-pick resolution is a
      // plain commit (`git_commit`). Accept either — the snapshot (no conflicts
      // left) gates actual completion.
      return op === "git_commit" || op === "git_update_branch";
    case "commit-push":
    case "fix-checks":
      return op === "git_commit" || op === "git_push";
    case "commit-pr":
      return op === "git_commit" || op === "open_pr";
    case "open-pr":
      return op === "open_pr";
    case "push":
      return op === "git_push";
    case "resolve-comments":
      // Any thread action proves the turn engaged with the review. Resolution
      // still gates completion (no threads left needing us), so accepting a bare
      // reply here is safe — and necessary, since a turn that only pushed back is
      // a legitimate outcome that resolves nothing.
      return op === "reply_thread" || op === "resolve_thread";
    case "update-branch":
      // Proven only by an actual base merge: a clean merge fires the clone's
      // post-merge hook, and a conflicted merge's completing commit is a merge
      // commit that post-commit also reports as `git_update_branch`. A plain
      // `git commit` reports `git_commit`, so an unrelated commit made during
      // this delegation can't stand in for the merge.
      return op === "git_update_branch";
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
export function delegationLabel(kind: DelegationKind): string {
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
    case "resolve-comments":
      return "Agent is working through the review comments…";
  }
}

/** Success notice once the watched transition lands. */
export function delegationDone(kind: DelegationKind): string {
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
    case "resolve-comments":
      return "Review comments addressed";
  }
}

/** Whether the transition this delegation is waiting for has landed. Pure —
 *  `useDelegationSync` evaluates it against every poll tick, using the state of
 *  the delegation's OWN checkout. `fix-checks` is the exception: CI re-runs take
 *  minutes, so the watcher resolves it as soon as the agent goes idle and lets
 *  the checks polling carry the story from there. */
export function delegationResolved(
  kind: DelegationKind,
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
      return pr?.mergeable === "mergeable";
    case "fix-checks":
      return false;
    case "resolve-comments":
      // Threads only clear once GitHub reports them resolved, which the slow
      // comments poll picks up. Like fix-checks, the caller resolves this on
      // agent-idle and lets the polling carry the story.
      return false;
  }
}
