// ── The one gate every panel action passes ────────────────────────────────────
// Why an action can't run right now, or null. Applied by `useFooterModel` to
// every action the footer emits — the primary and each menu row — and again by
// `runAction` before it dispatches, so nothing reaches the host that the
// button wouldn't: not a stale menu, not the composer's shortcut, not a row
// picked on a lane the action isn't about. Five rules, in the order a user
// would want to hear them.
//
// Store-free, so the phone runs the same gate over the same footer: the one
// thing that differs per client — which host it drives and what that host
// answers — comes in as `hostRefusal`.

export interface Gate {
  reason: string;
  /** Transient and already narrated by the status slot (a spinner, a label),
   *  so the footer disables without repeating it. */
  quiet?: boolean;
}

export interface GateContext {
  /** Why the host the client drives can't run `key`, or null: an older host
   *  may lack the op. The desktop asks `actionGateReason` for the active
   *  environment; the phone asks its own dispatch table. */
  hostRefusal: (key: string) => string | null;
  /** The head branch of the PR the lane is about; null when unknown. */
  laneBranch: string | null;
  /** The branch the checkout is on; null while unknown or detached. */
  checkoutBranch: string | null;
  /** GitHub's merge gate is open (`describeMergeGate`). */
  mergeAllowed: boolean;
  delegationActive: boolean;
  /** One of the panel's own actions is in flight on this checkout. */
  busy: boolean;
  /** The checkout is the agent's primary repo. */
  primaryCheckout: boolean;
  /** Uncommitted files or commits origin lacks since the PR merged — work
   *  that archiving the workspace or deleting the branch would take with it. */
  workSinceMerge: boolean;
}

/** The two ways to end a merged branch: both remove work only this checkout holds. */
const ENDS_BRANCH = new Set(["archive", "delete-branch"]);

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

/** Actions that touch neither the checkout nor its branch — a page on GitHub,
 *  the host's focus, the connect flow, the local reviewer — so a working
 *  agent is no reason to hold them. The reviewer only reads the tree, and a
 *  start while the coder is mid-turn (or a delegation is live) is queued by
 *  the host and runs when the coder goes idle, so clicking it then is right. */
const HARMLESS = new Set([
  "view-pr",
  "focus-pr",
  "connect-github",
  "review-start",
  "review-stop",
  "review-open",
]);

export function gateAction(key: string, ctx: GateContext): Gate | null {
  if (key === "loading") return { reason: "Loading git state", quiet: true };
  if (ctx.delegationActive && !HARMLESS.has(key)) {
    return { reason: "The agent is working on this checkout", quiet: true };
  }
  if (ctx.busy && !HARMLESS.has(key)) {
    return { reason: "An action is already running on this checkout", quiet: true };
  }
  const host = ctx.hostRefusal(key);
  if (host) return { reason: host };
  const scope = AGENT_SCOPED[key];
  if (scope && !ctx.primaryCheckout) return { reason: scope };
  if (BRANCH_BOUND.has(key) && ctx.laneBranch && ctx.checkoutBranch) {
    if (ctx.laneBranch !== ctx.checkoutBranch) {
      return {
        reason: `Checkout is on ${ctx.checkoutBranch}; this PR's branch is ${ctx.laneBranch}`,
      };
    }
  }
  if (ENDS_BRANCH.has(key) && ctx.workSinceMerge) {
    return { reason: "Work since the merge is only here" };
  }
  if (key === "merge" && !ctx.mergeAllowed) {
    return { reason: "GitHub's merge gate is closed — see the Pull request row" };
  }
  return null;
}
