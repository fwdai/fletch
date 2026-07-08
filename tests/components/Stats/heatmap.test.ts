import { describe, expect, it } from "vitest";

import { buildHeatmapWeeks, computeStreak, heatLevel, monthLabels } from "@/components/Stats";
import { localDay } from "@/util/format";

// A fixed reference point: Wed 2026-07-08, noon local.
const WED = new Date(2026, 6, 8, 12).getTime();
const DAY = 86_400_000;

describe("buildHeatmapWeeks", () => {
  it("builds weeks × 7 Monday-first columns ending in the current week", () => {
    const grid = buildHeatmapWeeks(WED, 4, {});
    expect(grid).toHaveLength(4);
    for (const col of grid) expect(col).toHaveLength(7);
    // Last column starts the Monday of the reference week…
    expect(grid[3][0].day).toBe("2026-07-06");
    // …and the first column starts three Mondays earlier.
    expect(grid[0][0].day).toBe("2026-06-15");
  });

  it("marks days after the reference point as out of range", () => {
    const grid = buildHeatmapWeeks(WED, 2, {});
    const last = grid[1];
    // Mon Jul 6 … Wed Jul 8 are real; Thu Jul 9 … Sun Jul 12 are padding.
    expect(last.map((c) => c.inRange)).toEqual([true, true, true, false, false, false, false]);
  });

  it("attaches counts by day key, defaulting to zero", () => {
    const grid = buildHeatmapWeeks(WED, 1, { "2026-07-07": 5 });
    expect(grid[0][1]).toMatchObject({ day: "2026-07-07", count: 5 });
    expect(grid[0][0].count).toBe(0);
  });
});

describe("monthLabels", () => {
  it("labels columns that enter a new month, plus a first column near a boundary", () => {
    // 6 weeks ending Jul 8: columns start Jun 1, 8, 15, 22, 29, Jul 6.
    const labels = monthLabels(buildHeatmapWeeks(WED, 6, {}));
    expect(labels).toEqual(["Jun", null, null, null, null, "Jul"]);
  });

  it("suppresses a mid-month label on the left edge", () => {
    const labels = monthLabels(buildHeatmapWeeks(WED, 4, {}));
    expect(labels).toEqual([null, null, null, "Jul"]);
  });
});

describe("heatLevel", () => {
  it("maps zero to level 0 and scales the rest to the max", () => {
    expect(heatLevel(0, 8)).toBe(0);
    expect(heatLevel(1, 8)).toBe(1);
    expect(heatLevel(4, 8)).toBe(2);
    expect(heatLevel(8, 8)).toBe(4);
  });

  it("never exceeds 4 and floors nonzero counts at 1", () => {
    expect(heatLevel(100, 8)).toBe(4);
    expect(heatLevel(1, 1000)).toBe(1);
  });
});

describe("computeStreak", () => {
  const days = (offsets: number[]) =>
    Object.fromEntries(offsets.map((o) => [localDay(WED - o * DAY), 1]));

  it("counts consecutive active days ending today", () => {
    expect(computeStreak(days([0, 1, 2]), WED)).toBe(3);
  });

  it("doesn't break the streak when today has no activity yet", () => {
    expect(computeStreak(days([1, 2, 3]), WED)).toBe(3);
  });

  it("stops at the first gap", () => {
    expect(computeStreak(days([0, 1, 3, 4]), WED)).toBe(2);
    expect(computeStreak(days([2, 3]), WED)).toBe(0);
  });

  it("is zero with no activity at all", () => {
    expect(computeStreak({}, WED)).toBe(0);
  });
});
