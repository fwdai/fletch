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

  it("renders nothing before the first git read", () => {
    expect(ladder({ git: null })).toEqual([]);
  });
});
