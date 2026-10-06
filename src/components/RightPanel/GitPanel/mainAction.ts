/** Whether the split button's main click can run, and why not when it can't.
 *
 *  Pure, and its own module, because it is the panel's one "disabled with a
 *  reason" rule: several things can kill the click and only two of them are
 *  worth words. `loading` and an in-flight delegation are transient, and the
 *  action bar's status slot is already narrating both; a capability gate is not
 *  going to clear on its own, and neither is a checkout on another branch than
 *  the focused PR a playbook would work on, so both have to say so. */
export interface MainActionState {
  disabled: boolean;
  /** The sentence to put beside the button and on its tooltip, or null when
   *  there is nothing a user could act on. */
  reason: string | null;
}

export function mainActionState(input: {
  /** The action the button would run (`useActionBarModel`'s `effectiveKey`). */
  effectiveKey: string;
  delegationActive: boolean;
  /** `describeMergeGate`'s verdict — review, checks and mergeability. Its own
   *  explanation is the PR card's, so it produces no reason here. */
  mergeAllowed: boolean;
  /** `actionGateReason(env, effectiveKey)` (./actionGates): why the environment
   *  the user is driving cannot be asked to run this action — a host from
   *  before its op, or an op withheld on policy — and null for every action it
   *  can. Already keyed by the action, so this is the whole capability story
   *  for the button, whichever key is selected. */
  actionGate: string | null;
  /** The focused PR's head branch while it is open, null otherwise (or while
   *  the host doesn't know it). */
  prBranch: string | null;
  /** The branch the checkout is on; null while unknown (loading, detached). */
  checkoutBranch: string | null;
}): MainActionState {
  const { effectiveKey, delegationActive, mergeAllowed, actionGate } = input;
  const mismatch = branchMismatch(input);
  return {
    disabled:
      effectiveKey === "loading" ||
      delegationActive ||
      actionGate !== null ||
      mismatch !== null ||
      (effectiveKey === "merge" && !mergeAllowed),
    reason: actionGate ?? mismatch,
  };
}

/** The agent playbooks that work on the focused PR itself — its failing checks,
 *  its stale base — rather than on whatever the checkout holds. Everything else
 *  (commit, push, pull, the `agent-commit*` modes, opening a PR) acts on the
 *  branch checked out, which is right whichever PR is focused; merge and view-pr
 *  act on GitHub alone. */
const PR_PLAYBOOKS = new Set(["agent-fix", "agent-update-branch"]);

/** Why the selected action would work on another branch than the PR it is
 *  about, or null. Once the user focuses an older PR of the checkout, the panel
 *  describes that PR while the working tree is still on another branch: fixing
 *  that PR's checks or updating its branch from here would land on the branch
 *  checked out. */
function branchMismatch(input: {
  effectiveKey: string;
  prBranch: string | null;
  checkoutBranch: string | null;
}): string | null {
  const { effectiveKey, prBranch, checkoutBranch } = input;
  if (!PR_PLAYBOOKS.has(effectiveKey)) return null;
  if (!prBranch || !checkoutBranch || prBranch === checkoutBranch) return null;
  return `Checkout is on ${checkoutBranch}; this PR's branch is ${prBranch}`;
}
