// The panel's one "disabled with a reason" rule: four things can kill the split
// button's main click, and only the capability gate is worth words.
//
// Which key maps to which gate is `actionGates.test.ts`'s subject; the reasons
// here are read through `actionGateReason` rather than spelled out, so this file
// holds no second copy of the wording and cannot drift from the table.

import { describe, expect, it } from "vitest";
import { V2_DEFAULT_OPS } from "@/remote/types";
import type { EnvironmentEntry } from "@/store/environments";
import { actionGateReason } from "./actionGates";
import { mainActionState } from "./mainAction";

/** A host from before the Git panel's ops — every gated key is closed on it. */
const oldHost: EnvironmentEntry = {
  id: "host-key-1",
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
  protocol: { version: 2, ops: [...V2_DEFAULT_OPS], events: [], features: [] },
};

const live = {
  effectiveKey: "merge",
  delegationActive: false,
  mergeAllowed: true,
  actionGate: null as string | null,
};

/** The button as this panel would build it for `key` against `oldHost`. */
const against = (key: string, rest: Partial<typeof live> = {}) =>
  mainActionState({
    ...live,
    ...rest,
    effectiveKey: key,
    actionGate: actionGateReason(oldHost, key),
  });

describe("mainActionState", () => {
  it("leaves the button live with nothing to explain", () => {
    expect(mainActionState(live)).toEqual({ disabled: false, reason: null });
  });

  it("explains a host that cannot merge the PR", () => {
    const state = against("merge");

    expect(state.disabled).toBe(true);
    expect(state.reason).toBe(actionGateReason(oldHost, "merge"));
  });

  it("explains a working-tree op the host is too old for", () => {
    // pull / rebase / stash / discard / abort are one class: on the wire now, so
    // closed only by an older host. One stands for the five — which key maps to
    // which gate is `actionGates.test.ts`'s subject.
    const state = against("pull");

    expect(state.disabled).toBe(true);
    expect(state.reason).toBe(actionGateReason(oldHost, "pull"));
  });

  it("explains an op withheld from every host on policy", () => {
    // `delete-branch` reaches outside the session's checkout, so no host
    // advertises it: closed against a current host too, not just an old one.
    const current: EnvironmentEntry = {
      ...oldHost,
      protocol: { version: 2, ops: [...V2_DEFAULT_OPS, "merge_pr"], events: [], features: [] },
    };
    const state = mainActionState({
      ...live,
      effectiveKey: "delete-branch",
      actionGate: actionGateReason(current, "delete-branch"),
    });

    expect(state.disabled).toBe(true);
    expect(state.reason).toBe(actionGateReason(current, "delete-branch"));
  });

  it("leaves an ungated action alone on the same old host", () => {
    // `push` has been on the wire since v1, and `agent-*` keys are the coding
    // agent's work, not an op — neither is the environment's business.
    expect(against("push")).toEqual({ disabled: false, reason: null });
    expect(against("agent-commit-push")).toEqual({ disabled: false, reason: null });
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
});
