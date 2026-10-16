import { describe, expect, it } from "vitest";
import type { GitState, PrSetEntry, PrState } from "@/api";
import { buildLanes, defaultLane } from "./laneModel";

function git(over: Partial<GitState> = {}): GitState {
  return {
    branch: "feat-a",
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
function pr(number: number, over: Partial<PrState> = {}): PrState {
  return {
    number,
    url: `https://x/${number}`,
    state: "open",
    title: `t${number}`,
    mergeable: "unknown",
    ...over,
  };
}
const entry = (state: PrState): PrSetEntry => ({ state, checks: null });

describe("buildLanes", () => {
  it("a checkout with no PR is one bare lane on the tree's branch", () => {
    const g = git();
    expect(buildLanes(undefined, null, null, g)).toEqual([
      { id: "branch:feat-a", branch: "feat-a", pr: null, current: true, focused: false, git: g },
    ]);
  });

  it("says the repo's remembered branch while the first git read is in flight", () => {
    const [lane] = buildLanes(undefined, null, null, null, undefined, "feat-a");
    expect(lane).toMatchObject({ id: "branch:feat-a", branch: "feat-a", current: true });
    // Once git has answered, the tree's own branch is the truth, even detached.
    const [read] = buildLanes(undefined, null, null, git({ branch: "" }), undefined, "feat-a");
    expect(read.branch).toBeNull();
  });

  it("a detached checkout is one lane with no branch yet", () => {
    const [lane] = buildLanes(undefined, null, null, git({ branch: "" }));
    expect(lane).toMatchObject({ id: "detached", branch: null, current: true });
  });

  it("one PR on the tree's branch is one lane, current and focused", () => {
    const p = pr(12, { branch: "feat-a" });
    const lanes = buildLanes([entry(p)], p, null, git());
    expect(lanes).toHaveLength(1);
    expect(lanes[0]).toMatchObject({ id: "pr:12", branch: "feat-a", current: true, focused: true });
  });

  it("a focused PR with no recorded branch is assumed to be on the tree's branch", () => {
    const p = pr(12);
    const [lane] = buildLanes([entry(p)], p, null, git());
    expect(lane).toMatchObject({ branch: "feat-a", current: true, focused: true });
  });

  it("orders the focused PR first, then the rest of the set", () => {
    const a = pr(12, { branch: "feat-a" });
    const b = pr(13, { branch: "feat-b" });
    const merged = pr(9, { branch: "chore", state: "merged" });
    const lanes = buildLanes([entry(b), entry(a), entry(merged)], a, null, git());
    expect(lanes.map((l) => l.id)).toEqual(["pr:12", "pr:13", "pr:9"]);
    expect(lanes.map((l) => l.current)).toEqual([true, false, false]);
    expect(lanes.map((l) => l.focused)).toEqual([true, false, false]);
  });

  it("adds a bare lane for the tree's branch when no PR covers it, after the focused one", () => {
    const b = pr(13, { branch: "feat-b" });
    const lanes = buildLanes([entry(b)], b, null, git({ branch: "feat-c" }));
    expect(lanes.map((l) => l.id)).toEqual(["pr:13", "branch:feat-c"]);
    expect(lanes[1]).toMatchObject({ current: true, focused: false, pr: null });
    expect(lanes[0].current).toBe(false);
  });

  it("marks exactly one lane current when several PRs share the tree's branch", () => {
    // A reused branch name: #9 merged on it, #12 open on it, #12 focused.
    const open12 = pr(12, { branch: "feat-a" });
    const merged9 = pr(9, { branch: "feat-a", state: "merged" });
    const lanes = buildLanes([entry(open12), entry(merged9)], open12, null, git());
    expect(lanes.map((l) => [l.id, l.current])).toEqual([
      ["pr:12", true],
      ["pr:9", false],
    ]);
    // Focused elsewhere: the newest open PR on the branch takes the tree.
    const other = pr(13, { branch: "feat-b" });
    const lanes2 = buildLanes([entry(open12), entry(merged9), entry(other)], other, null, git());
    expect(lanes2.filter((l) => l.current).map((l) => l.id)).toEqual(["pr:12"]);
    // Only history on the branch: the newest of it, never two.
    const lanes3 = buildLanes(
      [entry(merged9), entry(pr(8, { branch: "feat-a", state: "closed" }))],
      merged9,
      null,
      git(),
    );
    expect(lanes3.filter((l) => l.current).map((l) => l.id)).toEqual(["pr:9"]);
  });

  it("scopes a checkout's lanes to its repo in a multi-repo workspace", () => {
    const p = pr(12, { branch: "feat-a" });
    const scope = { id: "backend", subdir: "backend", label: "backend" };
    const lanes = buildLanes([entry(p)], p, null, git({ branch: "feat-c" }), scope);
    expect(lanes.map((l) => l.id)).toEqual(["backend/pr:12", "backend/branch:feat-c"]);
    expect(lanes.every((l) => l.scope === scope)).toBe(true);
  });

  it("defaults to the first lane: the focused PR's, else the tree's", () => {
    const b = pr(13, { branch: "feat-b" });
    const lanes = buildLanes([entry(b)], b, null, git({ branch: "feat-c" }));
    expect(defaultLane(lanes)?.id).toBe("pr:13");
    expect(defaultLane(buildLanes(undefined, null, null, git()))?.id).toBe("branch:feat-a");
    expect(defaultLane([])).toBeNull();
  });
});
