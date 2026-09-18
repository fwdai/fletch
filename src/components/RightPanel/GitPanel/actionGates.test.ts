// The decision `runAction` makes before it dispatches: can the environment the
// user is driving run this action at all? Same two rules as every other gate —
// never closed locally, closed on a remote host that doesn't answer the op —
// only reached by the panel's action key rather than a gate name, because that
// is what both the button and the dispatch hold.

import { describe, expect, it } from "vitest";
import { V2_DEFAULT_OPS } from "@/remote/types";
import { GATES } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";
import { ACTION_GATES, actionGateReason } from "./actionGates";

const local: EnvironmentEntry = {
  id: "local",
  name: "This Mac",
  kind: "local",
  connection: "connected",
};

const host = (ops?: string[]): EnvironmentEntry => ({
  id: "host-key-1",
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
  ...(ops ? { protocol: { version: 2, ops, events: [], features: [] } } : {}),
});

describe("actionGateReason", () => {
  it("refuses a gated action on a host that doesn't answer its op", () => {
    // A host from before the working-tree ops: it reported a descriptor, and
    // `stash_agent` is not on it.
    const old = host([...V2_DEFAULT_OPS, "pull_agent"]);

    expect(actionGateReason(old, "stash")).toBe(GATES.stash.reason);
    expect(actionGateReason(old, "discard")).toBe(GATES.discardChanges.reason);
    expect(actionGateReason(old, "abort")).toBe(GATES.abortMerge.reason);
    expect(actionGateReason(old, "rebase")).toBe(GATES.rebase.reason);
    // …and lets through the one it does answer.
    expect(actionGateReason(old, "pull")).toBeNull();
  });

  it("refuses every gated action on a host that reported no descriptor", () => {
    const old = host();

    for (const key of Object.keys(ACTION_GATES)) {
      expect(actionGateReason(old, key)).not.toBeNull();
    }
  });

  it("never refuses an action locally", () => {
    for (const key of Object.keys(ACTION_GATES)) {
      expect(actionGateReason(local, key)).toBeNull();
    }
  });

  it("passes through the keys that call no gated op, however old the host", () => {
    // Local actions, agent delegations and the v1 ops are nobody's gate, so the
    // dispatch has to reach them untouched on every environment.
    for (const key of ["commit-direct", "push", "open-pr", "agent-commit", "view-pr", "loading"]) {
      expect(actionGateReason(host(), key)).toBeNull();
      expect(actionGateReason(local, key)).toBeNull();
    }
  });

  it("opens the gated actions on a host that advertises their ops", () => {
    const current = host([
      ...V2_DEFAULT_OPS,
      "merge_pr",
      "pull_agent",
      "rebase_agent",
      "stash_agent",
      "discard_agent_changes",
      "abort_merge_agent",
    ]);

    for (const key of ["merge", "pull", "rebase", "stash", "discard", "abort"]) {
      expect(actionGateReason(current, key)).toBeNull();
    }
    // `delete_branch_agent` is withheld on policy — it writes in the user's
    // real clone — so no host advertises it and its action stays refused.
    expect(actionGateReason(current, "delete-branch")).toBe(GATES.deleteBranch.reason);
  });
});
