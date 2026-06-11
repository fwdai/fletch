import { describe, expect, it } from "vitest";
import type { GitState, PrChecks, PrState } from "../../api";
import {
  APP_ACTION_PREFIX,
  appActionMessage,
  delegationDone,
  delegationLabel,
  delegationResolved,
} from "./delegation";

function git(over: Partial<GitState> = {}): GitState {
  return {
    branch: "feat", parent_branch: "main", ahead: 1, behind: 0, unpushed: 0,
    files: [], additions: 0, deletions: 0, ...over,
  };
}
const file = (kind: GitState["files"][number]["kind"]) => ({
  path: "a.ts", kind, staged: false, additions: 1, deletions: 0,
});
function pr(over: Partial<PrState> = {}): PrState {
  return { number: 7, url: "https://x", state: "open", title: "t", mergeable: true, ...over };
}
function checks(merge_state: PrChecks["merge_state"]): PrChecks {
  return {
    merge_state, rollup: "none", total: 0, passed: 0, failed: 0, pending: 0,
    required_failing: [], runs: [],
  };
}

describe("delegationResolved", () => {
  it("commit resolves once the working tree is clean", () => {
    expect(delegationResolved("commit", git({ files: [file("modified")] }), null, null)).toBe(false);
    expect(delegationResolved("commit", git(), null, null)).toBe(true);
    expect(delegationResolved("commit", null, null, null)).toBe(false);
  });

  it("commit-push resolves once the tree is clean AND everything is pushed", () => {
    expect(delegationResolved("commit-push", git({ files: [file("modified")] }), null, null)).toBe(false);
    expect(delegationResolved("commit-push", git({ unpushed: 1 }), null, null)).toBe(false);
    expect(delegationResolved("commit-push", git(), null, null)).toBe(true);
  });

  it("commit-pr and open-pr resolve when a PR is open", () => {
    expect(delegationResolved("commit-pr", git(), null, null)).toBe(false);
    expect(delegationResolved("commit-pr", git(), pr(), null)).toBe(true);
    expect(delegationResolved("open-pr", git(), pr({ state: "closed" }), null)).toBe(false);
    expect(delegationResolved("open-pr", git(), pr(), null)).toBe(true);
  });

  it("resolve resolves when no conflicted files remain", () => {
    expect(delegationResolved("resolve", git({ files: [file("conflicted")] }), null, null)).toBe(false);
    expect(delegationResolved("resolve", git({ files: [file("modified")] }), null, null)).toBe(true);
  });

  it("update-branch waits out behind/dirty/unknown, falls back to mergeable", () => {
    expect(delegationResolved("update-branch", git(), pr(), checks("behind"))).toBe(false);
    expect(delegationResolved("update-branch", git(), pr(), checks("dirty"))).toBe(false);
    expect(delegationResolved("update-branch", git(), pr(), checks("unknown"))).toBe(false);
    expect(delegationResolved("update-branch", git(), pr(), checks("clean"))).toBe(true);
    expect(delegationResolved("update-branch", git(), pr({ mergeable: false }), null)).toBe(false);
    expect(delegationResolved("update-branch", git(), pr({ mergeable: true }), null)).toBe(true);
  });

  it("fix-checks never resolves from state (caller resolves on agent idle)", () => {
    expect(delegationResolved("fix-checks", git(), pr(), checks("clean"))).toBe(false);
  });
});

describe("appActionMessage", () => {
  it("builds a bare trigger without params", () => {
    expect(appActionMessage("commit")).toBe("[app-action] commit");
  });

  it("appends quoted key=value params", () => {
    expect(appActionMessage("commit-pr", { base: "main" })).toBe('[app-action] commit-pr base="main"');
    expect(appActionMessage("fix-checks", { failing: "build, test" })).toBe(
      '[app-action] fix-checks failing="build, test"',
    );
  });

  it("skips empty params and escapes embedded quotes", () => {
    expect(appActionMessage("open-pr", { base: "" })).toBe("[app-action] open-pr");
    expect(appActionMessage("fix-checks", { failing: 'say "hi"' })).toBe(
      '[app-action] fix-checks failing="say \\"hi\\""',
    );
  });

  it("triggers start with the shared prefix the transcript folds into a chip", () => {
    expect(appActionMessage("commit").startsWith(APP_ACTION_PREFIX)).toBe(true);
  });
});

describe("copy", () => {
  it("has a label and done message for every kind", () => {
    for (const k of ["commit", "commit-push", "commit-pr", "open-pr", "resolve", "update-branch", "fix-checks"] as const) {
      expect(delegationLabel(k).length).toBeGreaterThan(0);
      expect(delegationDone(k).length).toBeGreaterThan(0);
    }
  });
});
