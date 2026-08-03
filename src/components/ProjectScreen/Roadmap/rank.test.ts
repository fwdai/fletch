import { describe, expect, it } from "vitest";
import { dropIndex, planDrop, RANK_STEP, type Ranked, rankBetween } from "./rank";

const rows = (...ranks: number[]): Ranked[] => ranks.map((rank, i) => ({ id: `i${i}`, rank }));

describe("rankBetween", () => {
  it("splits the gap between two neighbours", () => {
    expect(rankBetween(1, 2)).toBe(1.5);
    expect(rankBetween(1, 1.5)).toBe(1.25);
    expect(rankBetween(-3, 5)).toBe(1);
  });

  it("steps past the edges rather than crowding them", () => {
    // Dropped at the top of a group: a whole step below the first rank, so the
    // next drag to the top has room again.
    expect(rankBetween(undefined, 1)).toBe(1 - RANK_STEP);
    // Dropped at the bottom.
    expect(rankBetween(4, undefined)).toBe(4 + RANK_STEP);
    // An empty group has no neighbours at all.
    expect(rankBetween(undefined, undefined)).toBe(RANK_STEP);
  });

  it("reports an unusable gap instead of returning a colliding rank", () => {
    // Two rows already sharing a rank leave nowhere between them.
    expect(rankBetween(2, 2)).toBeNull();
    // Out of order (a caller bug, or a board mid-refresh) is not a gap either.
    expect(rankBetween(3, 2)).toBeNull();
    // A gap halved until the float runs out: the midpoint *is* the lower end.
    const above = 1;
    const below = 1 + Number.EPSILON;
    expect((above + below) / 2).toBe(above);
    expect(rankBetween(above, below)).toBeNull();
  });
});

describe("planDrop", () => {
  it("writes one rank for the ordinary drop", () => {
    const group = rows(1, 2, 3);
    expect(planDrop(group, 1, "moved")).toEqual({ kind: "set", rank: 1.5 });
    expect(planDrop(group, 0, "moved")).toEqual({ kind: "set", rank: 0 });
    expect(planDrop(group, 3, "moved")).toEqual({ kind: "set", rank: 4 });
  });

  it("handles an empty group", () => {
    expect(planDrop([], 0, "moved")).toEqual({ kind: "set", rank: RANK_STEP });
  });

  it("clamps an index that has drifted out of range", () => {
    // The target row left the group mid-drag; the drop still lands.
    expect(planDrop(rows(1, 2), 9, "moved")).toEqual({ kind: "set", rank: 3 });
    expect(planDrop(rows(1, 2), -4, "moved")).toEqual({ kind: "set", rank: 0 });
  });

  it("renumbers the whole group when the gap holds nothing", () => {
    // Two rows share a rank 5 — nothing can be expressed between them, so the
    // group is rewritten from its own lowest rank instead.
    const plan = planDrop(rows(5, 5, 9), 1, "moved");
    expect(plan).toEqual({
      kind: "renumber",
      writes: [
        { id: "i0", rank: 5 },
        { id: "moved", rank: 6 },
        { id: "i1", rank: 7 },
        { id: "i2", rank: 8 },
      ],
    });
    // The dragged row is part of the rewrite, at the index it was dropped at,
    // and the sequence is strictly increasing — which is all the board's order
    // depends on.
    if (plan.kind !== "renumber") throw new Error("expected a renumber");
    const ranks = plan.writes.map((w) => w.rank);
    expect(ranks).toEqual([...ranks].sort((a, b) => a - b));
    expect(new Set(ranks).size).toBe(ranks.length);
  });

  it("never renumbers below the first slot", () => {
    // A group whose ranks are fractions below 1 still renumbers from 1, so no
    // write lands on a non-positive rank.
    const plan = planDrop(rows(0.25, 0.25), 1, "moved");
    if (plan.kind !== "renumber") throw new Error("expected a renumber");
    expect(plan.writes.map((w) => w.rank)).toEqual([1, 2, 3]);
  });
});

describe("dropIndex", () => {
  it("turns a target row and an edge into an insertion index", () => {
    const group = rows(1, 2, 3);
    expect(dropIndex(group, "i0", "before")).toBe(0);
    expect(dropIndex(group, "i0", "after")).toBe(1);
    expect(dropIndex(group, "i2", "after")).toBe(3);
    // A target that is no longer there appends rather than throwing.
    expect(dropIndex(group, "gone", "before")).toBe(3);
  });
});
