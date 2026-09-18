/** Whether the split button's main click can run, and why not when it can't.
 *
 *  Pure, and its own module, because it is the panel's one "disabled with a
 *  reason" rule: three things can kill the click and only one of them is worth
 *  words. `loading` and an in-flight delegation are transient, and the action
 *  bar's status slot is already narrating both; a capability gate is not going
 *  to clear on its own, so it has to say so. */
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
}): MainActionState {
  const { effectiveKey, delegationActive, mergeAllowed, actionGate } = input;
  return {
    disabled:
      effectiveKey === "loading" ||
      delegationActive ||
      actionGate !== null ||
      (effectiveKey === "merge" && !mergeAllowed),
    reason: actionGate,
  };
}
