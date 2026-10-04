// Whether a publish the backend is asking about was *already* authorized by
// something the user did.
//
// The backend gate (`rpc::approval`) asks about every publish when the setting is
// on. But some things already are an explicit user decision that entails
// publishing: enrolling a checkout in autopilot, starting a delegation (a Git-panel
// action), and launching a workflow (which publishes host-side and never reaches
// this path at all). Re-asking for those is either a double-confirm the user just
// made, or — for autopilot, which runs while nobody is watching — a stall.
//
// Delegations are the host's now: the gate itself approves a live delegation's own
// publishes without asking (`supervisor::delegation::pre_authorizes`). Autopilot
// enrollment is still this client's state, so its half of the policy lives here
// until autopilot moves to the host too. The trust boundary is unchanged — the
// backend still refuses unless something approves, and the webview is not
// agent-reachable.

import type { AutopilotState } from "@/autopilot";

/** The subset of store state the decision reads, so it can be tested as a
 *  function of its inputs rather than through the store. */
export interface PublishAuthorityState {
  autopilot: Record<string, AutopilotState>;
}

/** Whether autopilot is driving `key`, i.e. its enrollment is the user's standing
 *  consent to publish there. */
export function autopilotIsDriving(state: AutopilotState | undefined): boolean {
  return state?.enrolled === true;
}

/** Whether the user already authorized this publish: autopilot enrollment, for
 *  `git_push` only. This is the *unattended* case, so it is one boolean lookup
 *  and nothing more — a bug in a subtler predicate here would strand a run nobody
 *  is watching. Autopilot's rungs (`fix-checks`, `resolve`, `update-branch`,
 *  `resolve-comments`) all operate on a pull request that already exists, so it
 *  never needs `open_pr`; that stays promptable, which is the right asymmetry
 *  since opening a PR creates a new, often public artifact under the user's
 *  identity. */
export function publishPreAuthorized(
  op: string,
  key: string,
  state: PublishAuthorityState,
): boolean {
  return op === "git_push" && autopilotIsDriving(state.autopilot[key]);
}
