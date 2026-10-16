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
    expect(buildLanes(undefined, null, null, git())).toEqual([
      { id: "branch:feat-a", branch: "feat-a", pr: null, current: true, focused: false },
    ]);
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

  it("defaults to the focused lane, else the tree's", () => {
    const b = pr(13, { branch: "feat-b" });
    const lanes = buildLanes([entry(b)], b, null, git({ branch: "feat-c" }));
    expect(defaultLane(lanes)?.id).toBe("pr:13");
    expect(defaultLane(buildLanes(undefined, null, null, git()))?.id).toBe("branch:feat-a");
    expect(defaultLane([])).toBeNull();
  });
});
