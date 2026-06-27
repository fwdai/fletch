import type { AgentStatus, GitState, PrChecks, PrState } from "../../api";

/** One agent-delegated git action: the user clicked a panel action whose
 *  judgment part (message, description, conflict edits) belongs to the
 *  coding agent; the agent executes the mutation through the app's file
 *  RPC (`git_commit` / `open_pr` / `git_update_branch` / `git_push`). */
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
  /** Epoch ms when the delegation entered the current phase: set at send,
   *  reset on dequeue. The give-up grace window counts from here. */
  startedAt: number;
  /** OUR turn has been observed `running` since `startedAt`. Until then a
   *  settled status is pre-send state, not a finished delegation turn. */
  sawRunning: boolean;
  /** Was the target already satisfied at the instant our turn started running?
   *  If so, a current match is stale (a manual stash/discard while queued, or
   *  pre-existing clean/open state) — not something this turn performed. Only a
   *  match that was FALSE at run-start and later turned true is genuinely ours.
   *  Captured once, when `sawRunning` flips. */
  resolvedAtRunStart: boolean;
  /** The agent was mid-turn when the trigger was sent, so it's queued behind
   *  that turn. The foreign turn's running/settling must not arm or clear
   *  this delegation — it's waited out first, then `queued` drops. */
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
 *  - "dequeue": the pre-existing turn settled → drop `queued`, reset the clock
 *  - "mark-running": our turn started → set `sawRunning` + baseline `resolved`
 *  - "give-up": agent settled without the transition → clear + honest notice */
export type DelegationStep = "resolve" | "wait" | "dequeue" | "mark-running" | "give-up";

export function delegationStep(
  delegation: GitDelegation,
  status: AgentStatus,
  resolved: boolean,
  now: number,
): DelegationStep {
  // A snapshot match is OUR result only if our own turn ran AND the target
  // wasn't already satisfied when that turn started — i.e. we observed the
  // transition happen during the turn. `sawRunning` alone is not enough: a
  // target that matched before the turn (PR already open, tree cleaned by a
  // manual stash/discard while queued) would flip `sawRunning` on the first
  // `running` tick and resolve instantly without the agent doing the action.
  // `resolvedAtRunStart` is the baseline captured at that instant.
  if (resolved && delegation.sawRunning && !delegation.resolvedAtRunStart) return "resolve";
  const active = status === "running" || status === "spawning";
  // Queued behind a foreign turn: its activity is not ours to interpret.
  if (delegation.queued) return active ? "wait" : "dequeue";
  if (status === "running" && !delegation.sawRunning) return "mark-running";
  const armed = delegation.sawRunning || now - delegation.startedAt > DELEGATION_GIVE_UP_GRACE_MS;
  if (!active && armed) return "give-up";
  return "wait";
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
