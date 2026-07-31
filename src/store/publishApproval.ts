// Whether a publish the backend is asking about was *already* authorized by
// something the user did.
//
// The backend gate (`rpc::approval`) asks about every publish when the setting is
// on. But three things already are an explicit user decision that entails
// publishing: enrolling a checkout in autopilot, clicking a Git-panel action, and
// launching a workflow (which publishes host-side and never reaches this path at
// all). Re-asking for those is either a double-confirm the user just made, or — for
// autopilot, which runs while nobody is watching — a stall.
//
// The policy lives here rather than in Rust because the state it needs is here:
// autopilot enrollment and delegation liveness are both frontend-owned, keyed by
// `checkoutKey`. Mirroring them across the IPC boundary would duplicate the
// authority on what "the user asked for this" means. The trust boundary is
// unchanged — the backend still refuses unless something approves, and the webview
// is not agent-reachable.

import type { AutopilotState } from "@/autopilot";
import { actionProvesKind, type Delegation } from "@/delegation";

/** The subset of store state the decision reads, so it can be tested as a
 *  function of its inputs rather than through the store. */
export interface PublishAuthorityState {
  autopilot: Record<string, AutopilotState>;
  delegations: Record<string, Delegation>;
}

/** Whether autopilot is actively driving `key`, i.e. its enrollment is the user's
 *  standing consent to publish there.
 *
 *  A *paused* enrollment is not consent: autopilot dispatches nothing while
 *  paused, so a publish arriving then did not come from it. */
export function autopilotIsDriving(state: AutopilotState | undefined): boolean {
  return state?.enrolled === true && !state.paused;
}

/** Whether the user already authorized this publish.
 *
 *  Two independent grounds, deliberately unequal in complexity:
 *
 *  1. **Autopilot enrollment**, for `git_push` only. This is the *unattended*
 *     case, so it is one boolean lookup and nothing more — a bug in a subtler
 *     predicate here would strand a run nobody is watching. Autopilot's rungs
 *     (`fix-checks`, `resolve`, `update-branch`, `resolve-comments`) all operate
 *     on a pull request that already exists, so it never needs `open_pr`; that
 *     stays promptable, which is the right asymmetry since opening a PR creates a
 *     new, often public artifact under the user's identity.
 *
 *  2. **A live delegation on this same checkout** whose playbook includes this op
 *     — the user clicked "Commit & Push" or "Open PR" moments ago. Scoped to the
 *     same `key` and matched through `actionProvesKind`, so a delegation cannot
 *     launder an unrelated publish: a live `commit` delegation does not authorize
 *     a push. If this predicate is ever wrong the user is present and just
 *     clicked, so the cost is one redundant prompt, never a stall.
 */
export function publishPreAuthorized(
  op: string,
  key: string,
  state: PublishAuthorityState,
): boolean {
  if (op === "git_push" && autopilotIsDriving(state.autopilot[key])) return true;
  const kind = state.delegations[key]?.kind;
  return kind !== undefined && actionProvesKind(kind, op);
}
