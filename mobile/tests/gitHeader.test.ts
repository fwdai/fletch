// The Git tab's status strip in src/screens/Agent/GitTab/derive.ts.

import type { GitState } from "@desktop/api/types/git";
import type { PrChecks, PrState } from "@desktop/api/types/pr";
import { describe, expect, it } from "vitest";
import { describeGitHeader } from "../src/screens/Agent/GitTab/derive";

const git = (over: Partial<GitState> = {}): GitState => ({
  branch: "feat/x",
  parent_branch: "main",
  ahead: 0,
  behind: 0,
  unpushed: 0,
  files: [],
  additions: 0,
  deletions: 0,
  has_origin: true,
  ...over,
});

const file = (kind: GitState["files"][number]["kind"] = "modified") => ({
  path: "a.ts",
  kind,
  staged: false,
  additions: 1,
  deletions: 0,
});

const pr = (over: Partial<PrState> = {}): PrState => ({
  number: 7,
  url: "https://github.com/o/r/pull/7",
  state: "open",
  title: "x",
  mergeable: "unknown",
  ...over,
});

const checks = (over: Partial<PrChecks> = {}): PrChecks => ({
  merge_state: "clean",
  rollup: "passing",
  total: 1,
  passed: 1,
  failed: 0,
  pending: 0,
  required_failing: [],
  runs: [],
  ...over,
});

const head = (g: GitState | null, p: PrState | null = null, c: PrChecks | null = null) =>
  describeGitHeader(g, p, c, "feat/x", "main");

describe("describeGitHeader", () => {
  it("is neutral while the first read is in flight", () => {
    expect(head(null)).toEqual({ kind: "neutral", text: "Loading…" });
  });

  it("shows a dot and the base for a clean checkout", () => {
    expect(head(git())).toEqual({ kind: "clean", text: "feat/x", sub: "← main", dot: true });
  });

  it("says Uncommitted with the diff summary when files changed", () => {
    expect(head(git({ files: [file()] }))).toEqual({
      kind: "changes",
      pill: "Uncommitted",
      text: "feat/x",
      diff: true,
      prLink: false,
    });
  });

  it("keeps the open PR's link while new work takes over", () => {
    expect(head(git({ files: [file()] }), pr()).prLink).toBe(true);
  });

  it("says Pushed when commits are ahead of base and nothing is pending", () => {
    expect(head(git({ ahead: 2 }))).toEqual({
      kind: "info",
      pill: "Pushed",
      text: "feat/x",
      prLink: false,
    });
  });

  it("flags conflicts over everything else", () => {
    expect(head(git({ files: [file("conflicted")] }), pr())).toEqual({
      kind: "att",
      pill: "Conflicts",
      text: "feat/x",
      sub: "← main",
    });
  });

  it("names the PR and says ready when the gate is clean", () => {
    expect(head(git(), pr(), checks())).toEqual({
      kind: "ready",
      pill: "PR #7",
      text: "ready to merge",
      prLink: true,
    });
  });

  it("turns orange when checks are failing", () => {
    const h = head(git(), pr(), checks({ merge_state: "unstable", failed: 1, rollup: "failing" }));
    expect(h.kind).toBe("att");
    expect(h.text).toBe("checks failing");
    expect(h.pill).toBe("PR #7");
  });

  it("names the base when the PR is behind it", () => {
    const h = head(git(), pr(), checks({ merge_state: "behind" }));
    expect(h.kind).toBe("att");
    expect(h.text).toBe("behind main");
  });

  it("falls back to the coarse mergeable signal with no checks", () => {
    expect(head(git(), pr({ mergeable: "unknown" })).text).toBe("checking…");
  });

  it("is purple once the PR merged with nothing since", () => {
    expect(head(git({ ahead: 3 }), pr({ state: "merged" }))).toEqual({
      kind: "merged",
      pill: "Merged #7",
      text: "→ main",
      prLink: true,
    });
  });

  it("goes back to Uncommitted for follow-up work after a merge", () => {
    expect(head(git({ files: [file()] }), pr({ state: "merged" })).pill).toBe("Uncommitted");
  });

  it("is neutral for a closed PR", () => {
    expect(head(git(), pr({ state: "closed" }))).toEqual({
      kind: "neutral",
      pill: "Closed #7",
      text: "feat/x",
      prLink: true,
    });
  });

  it("says Git paused when the checkout's config blocks git, whatever else it reads", () => {
    expect(head(git({ files: [file()], blocked_config: ["core.hooksPath"] }), pr())).toEqual({
      kind: "att",
      pill: "Git paused",
      text: "blocking settings",
    });
  });
});
