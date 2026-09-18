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
  /** `gateReason(env, "mergePr")`: set only against a host from before the
   *  `merge_pr` op, where the click would come back `unknown op`. */
  mergePrGate: string | null;
}): MainActionState {
  const { effectiveKey, delegationActive, mergeAllowed, mergePrGate } = input;
  const merging = effectiveKey === "merge";
  return {
    disabled:
      effectiveKey === "loading" ||
      delegationActive ||
      (merging && (!mergeAllowed || mergePrGate !== null)),
    reason: merging ? mergePrGate : null,
  };
}
