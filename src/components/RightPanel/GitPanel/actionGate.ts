// ── The one gate every panel action passes ────────────────────────────────────
// Why an action can't run right now, or null. Applied by `useFooterModel` to
// every action the footer emits — the primary and each menu row — and again by
// `runAction` before it dispatches, so nothing reaches the host that the
// button wouldn't: not a stale menu, not the composer's shortcut, not a row
// picked on a lane the action isn't about. Five rules, in the order a user
// would want to hear them.

import type { EnvironmentEntry } from "@/store/environments";
import { actionGateReason } from "./actionGates";

export interface Gate {
  reason: string;
  /** Transient and already narrated by the status slot (a spinner, a label),
   *  so the footer disables without repeating it. */
  quiet?: boolean;
}

export interface GateContext {
  /** The environment the user is driving: an older host may lack the op. */
  env: EnvironmentEntry;
  /** The head branch of the PR the lane is about; null when unknown. */
  laneBranch: string | null;
  /** The branch the checkout is on; null while unknown or detached. */
  checkoutBranch: string | null;
  /** GitHub's merge gate is open (`describeMergeGate`). */
  mergeAllowed: boolean;
  delegationActive: boolean;
  /** The checkout is the agent's primary repo. */
  primaryCheckout: boolean;
}

/** The agent playbooks that work on the PR itself (its checks, its base, its
 *  threads) and the two ways to end a merged branch. From a tree on another
 *  branch than that PR's, these would land on the wrong one. Everything else
 *  acts on the branch checked out, which is right whichever lane is in view,
 *  or on GitHub alone. */
const BRANCH_BOUND = new Set([
  "agent-fix",
  "agent-update-branch",
  "agent-resolve-comments",
  "archive",
  "delete-branch",
]);

/** Actions whose backend works on the agent, not a checkout: publishing
 *  always publishes the primary repo, archiving ends the whole workspace. On a
 *  secondary repo's lane they would do something other than what the lane
 *  shows, so they wait for the primary's. */
const AGENT_SCOPED: Record<string, string> = {
  publish: "Only the workspace's primary repo can be published from here",
  "publish-public": "Only the workspace's primary repo can be published from here",
  archive: "Archive the workspace from its primary repo's lane",
};

export function gateAction(key: string, ctx: GateContext): Gate | null {
  if (key === "loading") return { reason: "Loading git state", quiet: true };
  if (ctx.delegationActive) {
    return { reason: "The agent is working on this checkout", quiet: true };
  }
  const env = actionGateReason(ctx.env, key);
  if (env) return { reason: env };
  const scope = AGENT_SCOPED[key];
  if (scope && !ctx.primaryCheckout) return { reason: scope };
  if (BRANCH_BOUND.has(key) && ctx.laneBranch && ctx.checkoutBranch) {
    if (ctx.laneBranch !== ctx.checkoutBranch) {
      return {
        reason: `Checkout is on ${ctx.checkoutBranch}; this PR's branch is ${ctx.laneBranch}`,
      };
    }
  }
  if (key === "merge" && !ctx.mergeAllowed) {
    return { reason: "GitHub's merge gate is closed — see the Pull request row" };
  }
  return null;
}
