// ── Delegation: handing one unit of work to the coding agent ───────────────
// A delegation is "the agent takes it from here" — the judgment part of an
// action (a commit message, a PR description, conflict edits, a test fix)
// belongs to the agent, and the host watches for the transition that proves it
// landed.
//
// The host owns the whole lifecycle (`crates/fletch-core/src/supervisor/
// delegation.rs`): it composes the trigger, holds it while the agent is
// mid-turn, decides when the work is done or abandoned, and pre-approves the
// delegation's own publishes. A client asks for one with `delegate_git` and
// renders what `delegation:changed` / `get_delegations` say — nothing here
// decides anything.

/** Every unit of work that can be handed to the coding agent. The playbook
 *  name the trigger carries is the kind itself, except `resolve`, whose
 *  playbook is `resolve-conflicts`.
 *
 *  Declared as a value, with the type derived from it, so tests that claim to
 *  cover "every kind" actually do: a hand-maintained list beside a union
 *  silently drifts. Mirrors `DelegationKind::ALL` on the host. */
export const DELEGATION_KINDS = [
  "commit",
  "commit-push",
  "commit-pr",
  "open-pr",
  "push",
  "resolve",
  "update-branch",
  "fix-checks",
  "resolve-comments",
] as const;

export type DelegationKind = (typeof DELEGATION_KINDS)[number];

/** Where a live delegation is, as the host reports it: held behind the agent's
 *  running turn, delivered, or its own turn under way. */
export type DelegationPhase = "queued" | "started" | "running";

/** A client's mirror of one live host delegation, keyed by
 *  `checkoutKey(agentId, subdir)` in the store. */
export interface Delegation {
  kind: DelegationKind;
  phase: DelegationPhase;
  /** Epoch ms when it entered its current phase (reset on delivery). */
  startedAt: number;
  /** The secondary repo it targets; undefined for the primary. */
  subdir?: string;
}

/** Marker prefix for app-sent action triggers. The full per-action playbooks
 *  live in the agent's injected instructions (`instructions/git_actions.md`),
 *  so the chat carries only this one-liner — which the transcript folds into
 *  a compact chip instead of a user bubble. */
export const APP_ACTION_PREFIX = "[app-action] ";

/** Build the one-line trigger: `[app-action] <name> key="value" …`. The host
 *  composes it for `delegate_git`; this copy is for a host from before that op,
 *  where the trigger still has to be sent as a plain message. Same format as
 *  the host's `app_action_message` — `tests/delegation.test.ts` and the Rust
 *  tests pin the same fixture. Empty values are dropped. */
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
