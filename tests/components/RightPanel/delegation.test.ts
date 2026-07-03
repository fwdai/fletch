import { describe, expect, it } from "vitest";
import type { GitState, PrChecks, PrState } from "@/api";
import {
  APP_ACTION_PREFIX,
  appActionMessage,
  DELEGATION_GIVE_UP_GRACE_MS,
  delegationDone,
  delegationLabel,
  delegationResolved,
  delegationStep,
  type GitDelegation,
  gitActionProvesKind,
} from "@/components/RightPanel/delegation";

function d(over: Partial<GitDelegation> = {}): GitDelegation {
  return {
    kind: "commit",
    prompt: "[app-action] commit",
    startedAt: 1_000,
    sawRunning: false,
    sawGitOp: false,
    queued: false,
    ...over,
  };
}

function git(over: Partial<GitState> = {}): GitState {
  return {
    branch: "feat",
    parent_branch: "main",
    ahead: 1,
    behind: 0,
    unpushed: 0,
    files: [],
    additions: 0,
    deletions: 0,
    has_origin: true,
    ...over,
  };
}
const file = (kind: GitState["files"][number]["kind"]) => ({
  path: "a.ts",
  kind,
  staged: false,
  additions: 1,
  deletions: 0,
});
function pr(over: Partial<PrState> = {}): PrState {
  return { number: 7, url: "https://x", state: "open", title: "t", mergeable: true, ...over };
}
function checks(merge_state: PrChecks["merge_state"]): PrChecks {
  return {
    merge_state,
    rollup: "none",
    total: 0,
    passed: 0,
    failed: 0,
    pending: 0,
    required_failing: [],
    runs: [],
  };
}

describe("delegationResolved", () => {
  it("commit resolves once the working tree is clean", () => {
    expect(delegationResolved("commit", git({ files: [file("modified")] }), null, null)).toBe(
      false,
    );
    expect(delegationResolved("commit", git(), null, null)).toBe(true);
    expect(delegationResolved("commit", null, null, null)).toBe(false);
  });

  it("commit-push resolves once the tree is clean AND everything is pushed", () => {
    expect(delegationResolved("commit-push", git({ files: [file("modified")] }), null, null)).toBe(
      false,
    );
    expect(delegationResolved("commit-push", git({ unpushed: 1 }), null, null)).toBe(false);
    expect(delegationResolved("commit-push", git(), null, null)).toBe(true);
  });

  it("commit-pr needs BOTH a clean tree and an open PR (existing PR isn't enough)", () => {
    expect(delegationResolved("commit-pr", git(), null, null)).toBe(false);
    // Reported bug: a PR is already open but new changes are still uncommitted —
    // must NOT resolve off the pre-existing PR.
    expect(delegationResolved("commit-pr", git({ files: [file("modified")] }), pr(), null)).toBe(
      false,
    );
    expect(delegationResolved("commit-pr", git(), pr(), null)).toBe(true);
  });

  it("open-pr resolves when a PR is open", () => {
    expect(delegationResolved("open-pr", git(), pr({ state: "closed" }), null)).toBe(false);
    expect(delegationResolved("open-pr", git(), pr(), null)).toBe(true);
  });

  it("resolve resolves when no conflicted files remain", () => {
    expect(delegationResolved("resolve", git({ files: [file("conflicted")] }), null, null)).toBe(
      false,
    );
    expect(delegationResolved("resolve", git({ files: [file("modified")] }), null, null)).toBe(
      true,
    );
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

describe("delegationStep", () => {
  const soon = 2_000; // within the grace window of startedAt=1_000
  const late = 1_000 + DELEGATION_GIVE_UP_GRACE_MS + 1;

  it("resolution requires the agent's own git op (sawGitOp), not just a match", () => {
    // The agent ran a mutating git op this turn AND the target is reached →
    // genuinely our result, whether the turn is still running or already idle.
    expect(delegationStep(d({ sawGitOp: true }), "idle", true, late)).toBe("resolve");
    expect(delegationStep(d({ sawGitOp: true }), "running", true, soon)).toBe("resolve");
  });

  it("a matching snapshot WITHOUT our git op never resolves (the reported bugs)", () => {
    // Target already satisfied but the agent did no git work — a manual
    // stash/discard, a pre-existing clean/open PR, or a foreign queued turn.
    // sawRunning alone (turn started) must NOT resolve; only sawGitOp does.
    expect(delegationStep(d({ sawRunning: true }), "running", true, soon)).toBe("wait");
    expect(delegationStep(d({ sawRunning: true }), "idle", true, late)).toBe("give-up");
    // Queued, target matches, but no agent git op recorded yet → not success.
    expect(delegationStep(d({ queued: true }), "running", true, soon)).toBe("wait");
    // Our turn hasn't even started: wait within grace, give up after.
    expect(delegationStep(d(), "idle", true, soon)).toBe("wait");
    expect(delegationStep(d(), "idle", true, late)).toBe("give-up");
  });

  it("a fast turn whose git op landed before resolve still succeeds (no false give-up)", () => {
    // sawGitOp arrives from the backend even if the snapshot/status timing is
    // tight — once set, a reached target resolves rather than giving up.
    expect(delegationStep(d({ sawGitOp: true }), "idle", true, soon)).toBe("resolve");
  });

  it("a still-queued delegation never resolves, even if a git op was recorded", () => {
    // Our trigger isn't delivered until the agent goes idle, so while `queued`
    // any git op belongs to the turn we're waiting behind. The store won't set
    // sawGitOp while queued; delegationStep also refuses to resolve a queued
    // delegation as belt-and-suspenders. It waits the turn out, then dequeues.
    expect(delegationStep(d({ queued: true, sawGitOp: true }), "running", true, soon)).toBe("wait");
    expect(delegationStep(d({ queued: true, sawGitOp: true }), "idle", true, late)).toBe("dequeue");
  });

  it("queued behind an in-flight turn: waits it out, then dequeues — never gives up", () => {
    // The pre-existing turn is still running: not ours, just wait.
    expect(delegationStep(d({ queued: true }), "running", false, late)).toBe("wait");
    // That turn settles: our trigger is next — dequeue, NOT "give-up", and
    // NOT "mark-running" off the foreign turn (the reported bug).
    expect(delegationStep(d({ queued: true }), "idle", false, late)).toBe("dequeue");
  });

  it("after dequeue, an idle gap before our turn starts is tolerated within the grace window", () => {
    // markGitDelegationDequeued resets startedAt, so `now` is near it again.
    expect(delegationStep(d(), "idle", false, soon)).toBe("wait");
    expect(delegationStep(d(), "idle", false, late)).toBe("give-up");
  });

  it("marks our own turn running exactly once, then settles into give-up", () => {
    expect(delegationStep(d(), "running", false, soon)).toBe("mark-running");
    expect(delegationStep(d({ sawRunning: true }), "running", false, late)).toBe("wait");
    expect(delegationStep(d({ sawRunning: true }), "idle", false, soon)).toBe("give-up");
  });

  it("spawning counts as active, not settled", () => {
    expect(delegationStep(d({ queued: true }), "spawning", false, late)).toBe("wait");
    expect(delegationStep(d({ sawRunning: true }), "spawning", false, late)).toBe("wait");
  });
});

describe("gitActionProvesKind", () => {
  it("accepts only ops that belong to the kind's own playbook", () => {
    expect(gitActionProvesKind("commit", "git_commit")).toBe(true);
    expect(gitActionProvesKind("push", "git_push")).toBe(true);
    expect(gitActionProvesKind("open-pr", "open_pr")).toBe(true);
    expect(gitActionProvesKind("update-branch", "git_update_branch")).toBe(true);
    expect(gitActionProvesKind("resolve", "git_commit")).toBe(true);
    // Multi-op kinds accept any op they touch (resolved gates real completion).
    expect(gitActionProvesKind("commit-push", "git_commit")).toBe(true);
    expect(gitActionProvesKind("commit-push", "git_push")).toBe(true);
    expect(gitActionProvesKind("commit-pr", "git_commit")).toBe(true);
    expect(gitActionProvesKind("commit-pr", "open_pr")).toBe(true);
  });

  it("rejects an unrelated op so a foreign queued turn can't prove the action", () => {
    // The reported bug: a `commit` delegation queued behind a turn that pushes
    // or opens a PR must NOT treat those as proof its commit ran.
    expect(gitActionProvesKind("commit", "git_push")).toBe(false);
    expect(gitActionProvesKind("commit", "open_pr")).toBe(false);
    expect(gitActionProvesKind("push", "git_commit")).toBe(false);
    expect(gitActionProvesKind("open-pr", "git_commit")).toBe(false);
    expect(gitActionProvesKind("update-branch", "git_push")).toBe(false);
  });
});

describe("appActionMessage", () => {
  it("builds a bare trigger without params", () => {
    expect(appActionMessage("commit")).toBe("[app-action] commit");
  });

  it("appends quoted key=value params", () => {
    expect(appActionMessage("commit-pr", { base: "main" })).toBe(
      '[app-action] commit-pr base="main"',
    );
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
    for (const k of [
      "commit",
      "commit-push",
      "commit-pr",
      "open-pr",
      "resolve",
      "update-branch",
      "fix-checks",
    ] as const) {
      expect(delegationLabel(k).length).toBeGreaterThan(0);
      expect(delegationDone(k).length).toBeGreaterThan(0);
    }
  });
});
