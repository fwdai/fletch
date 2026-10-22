import { describe, expect, it } from "vitest";
import type { GitState, PrChecks, PrComment, PrState } from "@/api";
import { buildLadder, type LadderInput, type LadderRow } from "./ladderModel";

function git(over: Partial<GitState> = {}): GitState {
  return {
    branch: "feat",
    parent_branch: "main",
    ahead: 0,
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
  return {
    number: 7,
    url: "https://x",
    state: "open",
    title: "t",
    mergeable: "mergeable",
    ...over,
  };
}
function checks(over: Partial<PrChecks> = {}): PrChecks {
  return {
    merge_state: "clean",
    rollup: "passing",
    total: 3,
    passed: 3,
    failed: 0,
    pending: 0,
    required_failing: [],
    runs: [],
    ...over,
  };
}
function thread(we_replied_last: boolean): PrComment {
  return {
    url: "https://x/1",
    author: "greptile",
    is_bot: true,
    body: "b",
    replies: 0,
    we_replied_last,
  } as PrComment;
}
function ladder(over: Partial<LadderInput>): LadderRow[] {
  return buildLadder({
    git: git(),
    pr: null,
    checks: null,
    mergeState: null,
    comments: null,
    base: "main",
    ...over,
  });
}
const row = (rows: LadderRow[], key: LadderRow["key"]) => rows.find((r) => r.key === key);
const current = (rows: LadderRow[]) => rows.find((r) => r.current)?.key ?? null;

describe("buildLadder", () => {
  it("a fresh checkout is three hollow rows with nothing current", () => {
    const rows = ladder({});
    expect(rows.map((r) => r.key)).toEqual(["changes", "commits", "pr"]);
    expect(rows.map((r) => r.mark)).toEqual(["idle", "idle", "idle"]);
    expect(current(rows)).toBeNull();
  });

  it("uncommitted work lights the changes row and makes it current", () => {
    const rows = ladder({ git: git({ files: [file("modified")] }) });
    expect(row(rows, "changes")).toMatchObject({ mark: "attention", tone: "warn", current: true });
    expect(row(rows, "changes")?.summary).toBe("1 file");
  });

  it("conflicts outrank plain changes", () => {
    const rows = ladder({ git: git({ files: [file("conflicted"), file("modified")] }) });
    expect(row(rows, "changes")).toMatchObject({ tone: "att", summary: "1 conflict in 2 files" });
  });

  it("keeps the PR rows on screen while the tree is dirty", () => {
    const rows = ladder({
      git: git({ files: [file("modified")], ahead: 2 }),
      pr: pr(),
      checks: checks({
        merge_state: "unstable",
        rollup: "failing",
        failed: 1,
        passed: 2,
        required_failing: ["test"],
      }),
      mergeState: "unstable",
      comments: { unresolved: [thread(false)] },
    });
    expect(rows.map((r) => r.key)).toEqual(["changes", "commits", "pr", "checks", "review"]);
    // The ladder's next step is the commit, so only that row is current …
    expect(current(rows)).toBe("changes");
    // … but the failing checks and the waiting thread still show their marks.
    expect(row(rows, "checks")).toMatchObject({
      mark: "attention",
      tone: "danger",
      current: false,
    });
    expect(row(rows, "review")).toMatchObject({
      summary: "1 thread waiting on you",
      current: false,
    });
  });

  it("unpushed commits on an open PR make the push the current step", () => {
    const rows = ladder({
      git: git({ ahead: 3, unpushed: 1 }),
      pr: pr(),
      checks: checks(),
      mergeState: "clean",
    });
    expect(current(rows)).toBe("commits");
    expect(row(rows, "commits")?.summary).toBe("3 ahead of main · 1 not pushed");
    expect(row(rows, "changes")).toMatchObject({ mark: "done", summary: "all committed" });
  });

  it("pushed commits with no PR point at opening one", () => {
    const rows = ladder({ git: git({ ahead: 2 }) });
    expect(row(rows, "commits")).toMatchObject({
      mark: "done",
      summary: "2 ahead of main · pushed",
    });
    expect(row(rows, "pr")).toMatchObject({ mark: "attention", tone: "info", current: true });
    expect(row(rows, "pr")?.summary).toBe("none yet · ready to open");
  });

  it("a green PR is done everywhere and the merge decision is the PR row", () => {
    const rows = ladder({
      git: git({ ahead: 2 }),
      pr: pr(),
      checks: checks(),
      mergeState: "clean",
      comments: { unresolved: [] },
    });
    expect(rows.every((r) => r.mark === "done")).toBe(true);
    expect(row(rows, "pr")?.summary).toBe("#7 · ready to merge");
    expect(current(rows)).toBe("pr");
  });

  it("failing checks are the current step once the tree is committed and pushed", () => {
    const rows = ladder({
      git: git({ ahead: 2 }),
      pr: pr(),
      checks: checks({
        merge_state: "blocked",
        rollup: "failing",
        failed: 2,
        passed: 1,
        required_failing: ["a", "b"],
      }),
      mergeState: "blocked",
      comments: { unresolved: [] },
    });
    expect(current(rows)).toBe("checks");
    expect(row(rows, "checks")?.summary).toBe("2 failing · 1 passed");
    // The PR row is about existence and sync, not CI: a failing PR is still "open".
    expect(row(rows, "pr")).toMatchObject({ mark: "done", summary: "#7 · open" });
  });

  it("a PR behind its base is the PR row's problem", () => {
    const rows = ladder({
      git: git({ ahead: 2 }),
      pr: pr(),
      checks: checks({ merge_state: "behind" }),
      mergeState: "behind",
    });
    expect(row(rows, "pr")).toMatchObject({
      mark: "attention",
      tone: "att",
      summary: "#7 · behind main",
      current: true,
    });
  });

  it("a pure review gate escalates on the review row", () => {
    const rows = ladder({
      git: git({ ahead: 2 }),
      pr: pr(),
      checks: checks({ merge_state: "blocked", rollup: "passing" }),
      mergeState: "blocked",
      comments: { unresolved: [] },
    });
    expect(row(rows, "review")).toMatchObject({ summary: "approval required", current: true });
    expect(row(rows, "checks")?.mark).toBe("done");
  });

  it("threads we already answered await the other side, not us", () => {
    const rows = ladder({
      git: git({ ahead: 2 }),
      pr: pr(),
      checks: checks(),
      mergeState: "clean",
      comments: { unresolved: [thread(true), thread(true)] },
    });
    expect(row(rows, "review")).toMatchObject({
      tone: "info",
      summary: "2 threads awaiting their reply",
    });
  });

  it("checks still being read show as running, not missing", () => {
    const rows = ladder({ git: git({ ahead: 1 }), pr: pr(), checks: null, mergeState: "unknown" });
    expect(row(rows, "checks")).toMatchObject({ mark: "running", summary: "checking…" });
  });

  it("a merged PR stays a row, and follow-up work climbs above it", () => {
    const landed = ladder({ git: git({ ahead: 3 }), pr: pr({ state: "merged" }) });
    expect(landed.map((r) => r.key)).toEqual(["changes", "commits", "pr"]);
    expect(row(landed, "pr")).toMatchObject({
      mark: "done",
      tone: "merged",
      summary: "#7 · merged into main",
      current: true,
    });
    expect(row(landed, "commits")?.summary).toBe("landed in #7");

    // New commits after the merge: the next step is a follow-up PR (which also
    // pushes), so the PR row is current and the commits row only lit.
    const followUp = ladder({ git: git({ ahead: 4, unpushed: 1 }), pr: pr({ state: "merged" }) });
    expect(row(followUp, "commits")).toMatchObject({
      mark: "attention",
      summary: "1 commit since #7 · not pushed",
      current: false,
    });
    expect(row(followUp, "pr")).toMatchObject({
      mark: "attention",
      tone: "info",
      summary: "#7 merged · follow-up ready",
      current: true,
    });
  });

  it("a closed PR is the ladder's problem to escalate", () => {
    const rows = ladder({ git: git({ ahead: 1 }), pr: pr({ state: "closed" }) });
    expect(row(rows, "pr")).toMatchObject({
      mark: "attention",
      tone: "neutral",
      summary: "#7 · closed",
      current: true,
    });
  });

  it("says where the changes are when the tree is on another branch than the PR", () => {
    const rows = ladder({
      git: git({ branch: "feat-b", files: [file("modified")] }),
      pr: pr({ branch: "feat-a" }),
    });
    expect(row(rows, "changes")?.summary).toBe("1 file on feat-b");
  });

  it("keeps the same rows for a lane only looked at, and says why they're idle", () => {
    // Another PR of the checkout: the tree is on a different branch, its
    // threads are not read, nothing is the next step.
    const rows = ladder({
      git: git({ branch: "feat-a", files: [file("modified")], ahead: 2 }),
      pr: pr({ branch: "feat-b" }),
      checks: checks({ rollup: "failing", failed: 1, passed: 2, merge_state: "unstable" }),
      mergeState: "unstable",
      lane: { tree: false, focused: false },
    });
    expect(rows.map((r) => r.key)).toEqual(["changes", "commits", "pr", "checks", "review"]);
    expect(row(rows, "changes")).toMatchObject({
      mark: "idle",
      summary: "working tree is on feat-a",
    });
    expect(row(rows, "commits")).toMatchObject({ mark: "idle", summary: "not checked out" });
    // What the set entry knows about the PR still shows with its marks.
    expect(row(rows, "pr")?.summary).toBe("#7 · open");
    expect(row(rows, "checks")).toMatchObject({ mark: "attention", tone: "danger" });
    expect(row(rows, "review")).toMatchObject({ mark: "idle", summary: "read for the focused PR" });
    expect(current(rows)).toBeNull();
  });

  it("renders nothing before the first git read", () => {
    expect(ladder({ git: null })).toEqual([]);
  });
});

// ── The local reviewer ──────────────────────────────────────────────────────
// The Review row carries both sources — GitHub's threads and the reviewer's
// findings — and, between them, where the reviewer itself stands.

import type { ReviewState } from "@/api";

const NOW = 1_700_000_000_000;

function review(over: Partial<ReviewState> = {}): ReviewState {
  return {
    agent_id: "fuji",
    subdir: null,
    phase: "clean",
    trigger: "manual",
    reviewer: {
      custom_agent_id: "ca-1",
      name: "Reviewer",
      provider: "codex",
      model: null,
      agent_id: "fuji-reviewer",
    },
    round: 1,
    open: 0,
    resolved: 0,
    since_ms: NOW - 5 * 60_000,
    reviewed_signature: "sha",
    error: null,
    ...over,
  };
}
const finding = (we_replied_last = false): PrComment =>
  ({
    id: "local:1",
    url: null,
    author: "Reviewer",
    is_bot: true,
    body: "b",
    replies: 0,
    we_replied_last,
    source: "local",
    severity: "high",
  }) as PrComment;

describe("buildLadder · the local reviewer", () => {
  it("adds a Review row before any PR once the host can review and there is work", () => {
    expect(ladder({ git: git({ ahead: 1 }) }).map((r) => r.key)).toEqual([
      "changes",
      "commits",
      "pr",
    ]);
    const rows = ladder({ git: git({ ahead: 1 }), canReview: true });
    expect(rows.map((r) => r.key)).toEqual(["changes", "commits", "pr", "review"]);
    expect(row(rows, "review")).toMatchObject({ mark: "idle", summary: "not reviewed" });
    // A fresh checkout with nothing to review keeps its three rows.
    expect(ladder({ canReview: true }).map((r) => r.key)).toEqual(["changes", "commits", "pr"]);
  });

  it("shows a running review with its timer", () => {
    const rows = ladder({
      git: git({ ahead: 1 }),
      review: review({ phase: "reviewing", since_ms: NOW - 83_000 }),
      nowMs: NOW,
    });
    expect(row(rows, "review")).toMatchObject({
      mark: "running",
      summary: "Reviewing · Reviewer · 1:23",
    });
  });

  it("counts the reviewer's findings as waiting on you, with the round, and makes them current", () => {
    const rows = ladder({
      git: git({ ahead: 1 }),
      review: review({ phase: "findings", round: 2, open: 2 }),
      comments: { unresolved: [finding(), finding()] },
    });
    expect(row(rows, "review")).toMatchObject({
      mark: "attention",
      tone: "att",
      summary: "2 findings waiting on you · round 2",
      current: true,
    });
    // Pre-PR, the findings come before the proposal: the PR row isn't current.
    expect(row(rows, "pr")?.current).toBe(false);
  });

  it("reads clean with the reviewer's name and the verdict's age", () => {
    const rows = ladder({ git: git({ ahead: 1 }), review: review(), nowMs: NOW });
    expect(row(rows, "review")).toMatchObject({
      mark: "done",
      summary: "Clean · Reviewer, 5m ago",
    });
    const fixed = ladder({
      git: git({ ahead: 1 }),
      review: review({ resolved: 3, since_ms: NOW }),
      nowMs: NOW,
    });
    expect(row(fixed, "review")?.summary).toBe("Clean · Reviewer, just now · 3 fixed");
  });

  it("says when the code moved on since the review, and when the review failed", () => {
    const stale = ladder({ git: git({ ahead: 2 }), review: review({ phase: "stale" }) });
    expect(row(stale, "review")).toMatchObject({
      mark: "attention",
      tone: "info",
      summary: "Stale · code changed since review",
    });
    // The code moved during the review itself: the host's note replaces the
    // generic one.
    const during = ladder({
      git: git({ ahead: 2 }),
      review: review({ phase: "stale", error: "the code changed during the review" }),
    });
    expect(row(during, "review")?.summary).toBe("Stale · the code changed during the review");
    const failed = ladder({
      git: git({ ahead: 2 }),
      review: review({ phase: "failed", error: "no verdict" }),
    });
    expect(row(failed, "review")).toMatchObject({
      tone: "danger",
      summary: "Review failed · no verdict",
    });
  });

  it("shows a queued review as waiting for the coder, muted and without a timer", () => {
    const rows = ladder({
      git: git({ ahead: 1 }),
      review: review({ phase: "queued", since_ms: NOW - 83_000 }),
      nowMs: NOW,
      coderName: "Claude",
    });
    expect(row(rows, "review")).toMatchObject({
      mark: "running",
      tone: "neutral",
      summary: "Queued · waiting for Claude to finish",
    });
    // Without a name for the coder it still reads.
    const unnamed = ladder({ git: git({ ahead: 1 }), review: review({ phase: "queued" }) });
    expect(row(unnamed, "review")?.summary).toBe("Queued · waiting for the agent to finish");
  });

  it("keeps GitHub's threads on the same row once a PR is open", () => {
    const rows = ladder({
      git: git({ ahead: 1 }),
      pr: pr(),
      checks: checks(),
      mergeState: "clean",
      review: review(),
      comments: { unresolved: [thread(false), finding()] },
    });
    expect(row(rows, "review")?.summary).toBe("2 findings waiting on you · round 1");
    expect(row(rows, "checks")?.mark).toBe("done");
  });
});
