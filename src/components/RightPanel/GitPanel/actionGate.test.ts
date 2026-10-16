// One gate for every action the footer offers — the primary and each menu row
// alike — and the same gate the dispatch re-runs. Which key maps to which host
// op is `actionGates.test.ts`'s subject; here the environment reasons are read
// through `actionGateReason` so the wording has one home.

import { describe, expect, it } from "vitest";
import { V2_DEFAULT_OPS } from "@/remote/types";
import type { EnvironmentEntry } from "@/store/environments";
import { type GateContext, gateAction } from "./actionGate";
import { actionGateReason } from "./actionGates";

const local: EnvironmentEntry = {
  id: "local",
  name: "This Mac",
  kind: "local",
  connection: "connected",
};
/** A host from before the Git panel's ops — every gated key is closed on it. */
const oldHost: EnvironmentEntry = {
  id: "host-key-1",
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
  protocol: { version: 2, ops: [...V2_DEFAULT_OPS], events: [], features: [] },
};

const open: GateContext = {
  env: local,
  laneBranch: "feat/first",
  checkoutBranch: "feat/first",
  mergeAllowed: true,
  delegationActive: false,
  primaryCheckout: true,
  workSinceMerge: false,
};
const reason = (key: string, over: Partial<GateContext> = {}) =>
  gateAction(key, { ...open, ...over })?.reason ?? null;

describe("gateAction", () => {
  it("leaves an action alone with nothing against it", () => {
    for (const key of ["agent-commit", "push", "merge", "agent-fix", "archive", "focus-pr"]) {
      expect(gateAction(key, open)).toBeNull();
    }
  });

  it("explains a host that cannot run the op, for any key", () => {
    for (const key of ["merge", "pull", "agent-commit-push", "focus-pr"]) {
      expect(reason(key, { env: oldHost })).toBe(actionGateReason(oldHost, key));
    }
    // `push` has been on the wire since v1 — not the environment's business.
    expect(reason("push", { env: oldHost })).toBeNull();
  });

  it("keeps a PR-bound action off a tree on another branch, wherever it is offered", () => {
    // The user focused an older PR; the tree is still on the newer one's branch.
    const elsewhere = { laneBranch: "feat/first", checkoutBranch: "feat/second" };
    const why = "Checkout is on feat/second; this PR's branch is feat/first";
    for (const key of [
      "agent-fix",
      "agent-update-branch",
      "agent-resolve-comments",
      "archive",
      "delete-branch",
    ]) {
      expect(reason(key, elsewhere)).toBe(why);
    }
    // Work on the checkout itself acts on the branch checked out; merge and
    // view-pr act on GitHub alone.
    for (const key of ["agent-commit", "agent-commit-push", "push", "pull", "merge", "view-pr"]) {
      expect(reason(key, elsewhere)).toBeNull();
    }
    // One side unknown: nothing to say.
    expect(reason("agent-fix", { laneBranch: null })).toBeNull();
    expect(reason("agent-fix", { checkoutBranch: null })).toBeNull();
  });

  it("keeps agent-scoped actions on the primary repo's lane", () => {
    for (const key of ["publish", "publish-public", "archive"]) {
      expect(reason(key, { primaryCheckout: false })).toMatch(/primary repo/);
      expect(reason(key)).toBeNull();
    }
    // A secondary's own branch can still be pushed, pulled and deleted.
    for (const key of ["push", "pull", "delete-branch", "agent-commit-pr"]) {
      expect(reason(key, { primaryCheckout: false })).toBeNull();
    }
  });

  it("keeps a merged branch's ends apart: scope gates archive, not delete-branch", () => {
    // A secondary repo's merged PR: the workspace is archived from the primary,
    // but this checkout's own branch can still be deleted.
    expect(reason("archive", { primaryCheckout: false })).toMatch(/primary repo/);
    expect(reason("delete-branch", { primaryCheckout: false })).toBeNull();
    // Work since the merge that only this checkout holds stops both.
    for (const key of ["archive", "delete-branch"]) {
      expect(reason(key, { workSinceMerge: true })).toMatch(/since the merge/);
    }
    expect(reason("push", { workSinceMerge: true })).toBeNull();
  });

  it("closes Merge with the gate, loudly enough for a menu row's tooltip", () => {
    expect(reason("merge", { mergeAllowed: false })).toMatch(/merge gate/);
    expect(gateAction("merge", { ...open, mergeAllowed: false })?.quiet).toBeUndefined();
  });

  it("is quiet about the transient blocks the bar already narrates", () => {
    expect(gateAction("loading", open)).toMatchObject({ quiet: true });
    expect(gateAction("agent-commit", { ...open, delegationActive: true })).toMatchObject({
      quiet: true,
    });
  });
});
