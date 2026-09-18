// The panel's one "disabled with a reason" rule: three things can kill the
// split button's main click, and only the capability gate is worth words.

import { describe, expect, it } from "vitest";
import { GATES } from "@/store/capabilities";
import { mainActionState } from "./mainAction";

const live = {
  effectiveKey: "merge",
  delegationActive: false,
  mergeAllowed: true,
  mergePrGate: null,
};

describe("mainActionState", () => {
  it("leaves the button live with nothing to explain", () => {
    expect(mainActionState(live)).toEqual({ disabled: false, reason: null });
  });

  it("explains a host that cannot merge at all", () => {
    const state = mainActionState({ ...live, mergePrGate: GATES.mergePr.reason });

    expect(state.disabled).toBe(true);
    expect(state.reason).toBe(GATES.mergePr.reason);
  });

  it("stays silent about the transient blocks the bar already narrates", () => {
    // Loading git state, and the agent holding a delegation: both clear on their
    // own, and the status slot is showing a spinner and a label for each.
    expect(mainActionState({ ...live, effectiveKey: "loading" })).toEqual({
      disabled: true,
      reason: null,
    });
    expect(mainActionState({ ...live, delegationActive: true })).toEqual({
      disabled: true,
      reason: null,
    });
  });

  it("stays silent about the merge gate, whose reasons the PR card carries", () => {
    expect(mainActionState({ ...live, mergeAllowed: false })).toEqual({
      disabled: true,
      reason: null,
    });
  });

  it("says nothing on an action the gate has no bearing on", () => {
    // The gate is about `merge_pr`; pushing is on every host's op table, so a
    // closed merge gate must not disable or annotate the push button.
    const state = mainActionState({
      ...live,
      effectiveKey: "push",
      mergeAllowed: false,
      mergePrGate: GATES.mergePr.reason,
    });

    expect(state).toEqual({ disabled: false, reason: null });
  });
});
