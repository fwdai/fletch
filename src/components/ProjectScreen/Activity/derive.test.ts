import { describe, expect, it } from "vitest";
import {
  dailySpend,
  mergeStats,
  type PrRow,
  recentDays,
  recentWeeks,
  type UsageRow,
} from "./derive";

// Local-midnight epoch for a YYYY-MM-DD day, so fixtures read as dates and the
// bucketing under test sees the same local calendar the app runs on.
const at = (day: string, hour = 12): number => {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d, hour).getTime();
};

const HOUR = 3_600_000;

// ── calendar ranges ───────────────────────────────────────────────────────

describe("recentDays", () => {
  it("returns n days oldest-first, ending today", () => {
    expect(recentDays(at("2026-03-05"), 4)).toEqual([
      "2026-03-02",
      "2026-03-03",
      "2026-03-04",
      "2026-03-05",
    ]);
  });

  it("steps across a month boundary", () => {
    expect(recentDays(at("2026-03-02"), 3)).toEqual(["2026-02-28", "2026-03-01", "2026-03-02"]);
  });
});

describe("recentWeeks", () => {
  it("returns Mondays oldest-first, ending with this week", () => {
    // 2026-03-05 is a Thursday; its week starts Monday the 2nd.
    expect(recentWeeks(at("2026-03-05"), 3)).toEqual(["2026-02-16", "2026-02-23", "2026-03-02"]);
  });

  it("treats Sunday as the end of its week, not the start of the next", () => {
    // 2026-03-08 is a Sunday — still the week of Monday the 2nd.
    expect(recentWeeks(at("2026-03-08"), 1)).toEqual(["2026-03-02"]);
  });
});

// ── time to merge ─────────────────────────────────────────────────────────

const pr = (over: Partial<PrRow> = {}): PrRow => ({
  opened_at: at("2026-03-02"),
  merged_at: at("2026-03-02") + 2 * HOUR,
  state: "merged",
  ...over,
});

describe("mergeStats", () => {
  it("takes the middle duration for an odd count", () => {
    const rows = [1, 5, 3].map((h) =>
      pr({ opened_at: at("2026-03-02"), merged_at: at("2026-03-02") + h * HOUR }),
    );
    expect(mergeStats(rows, at("2026-03-05"), 1).medianMs).toBe(3 * HOUR);
  });

  it("averages the two middle durations for an even count", () => {
    const rows = [1, 2, 4, 9].map((h) =>
      pr({ opened_at: at("2026-03-02"), merged_at: at("2026-03-02") + h * HOUR }),
    );
    expect(mergeStats(rows, at("2026-03-05"), 1).medianMs).toBe(3 * HOUR);
  });

  it("reports the fastest merge alongside the median", () => {
    const rows = [8, 1, 4].map((h) =>
      pr({ opened_at: at("2026-03-02"), merged_at: at("2026-03-02") + h * HOUR }),
    );
    expect(mergeStats(rows, at("2026-03-05"), 1).fastestMs).toBe(1 * HOUR);
  });

  it("has no median until something merges", () => {
    const s = mergeStats([pr({ merged_at: null, state: "open" })], at("2026-03-05"), 1);
    expect(s.medianMs).toBeNull();
    expect(s.fastestMs).toBeNull();
    expect(s.merged).toBe(0);
    expect(s.open).toBe(1);
  });

  it("counts a back-seeded row as merged but takes no duration from it", () => {
    // Migration 0025 seeded rows whose merge time can predate the open time it
    // also seeded; that is a data artifact, not a negative-lifetime PR.
    const rows = [
      pr({ opened_at: at("2026-03-03"), merged_at: at("2026-03-02") }),
      pr({ opened_at: at("2026-03-02"), merged_at: at("2026-03-02") + 6 * HOUR }),
    ];
    const s = mergeStats(rows, at("2026-03-05"), 1);
    expect(s.merged).toBe(2);
    expect(s.medianMs).toBe(6 * HOUR);
  });

  it("counts a PR with no observed open time as merged", () => {
    const s = mergeStats([pr({ opened_at: null })], at("2026-03-05"), 1);
    expect(s.merged).toBe(1);
    expect(s.medianMs).toBeNull();
  });

  it("buckets opened and merged into their own weeks", () => {
    // Opened in the week of Feb 23, merged in the week of Mar 2.
    const rows = [pr({ opened_at: at("2026-02-25"), merged_at: at("2026-03-03") })];
    const weeks = mergeStats(rows, at("2026-03-05"), 3).weeks;
    expect(weeks.map((w) => [w.start, w.opened, w.merged])).toEqual([
      ["2026-02-16", 0, 0],
      ["2026-02-23", 1, 0],
      ["2026-03-02", 0, 1],
    ]);
  });

  it("reports a quiet week as a real zero", () => {
    // The PR log is complete, so no rows genuinely means no PRs — unlike the
    // usage series, absence here is knowledge and must plot as 0, not a gap.
    const weeks = mergeStats([], at("2026-03-05"), 2).weeks;
    expect(weeks.every((w) => w.opened === 0 && w.merged === 0)).toBe(true);
    expect(weeks).toHaveLength(2);
  });
});

// ── spend over time ───────────────────────────────────────────────────────

const usage = (workspace_id: string, day: string, tokens: number, cost = 0): UsageRow => ({
  workspace_id,
  day,
  tokens,
  cost,
});

const DAYS = ["2026-03-01", "2026-03-02", "2026-03-03", "2026-03-04"];

describe("dailySpend", () => {
  it("charts the difference between consecutive snapshots, not the total", () => {
    const rows = [
      usage("w1", "2026-03-01", 100),
      usage("w1", "2026-03-02", 250),
      usage("w1", "2026-03-03", 300),
    ];
    expect(dailySpend(rows, DAYS).map((d) => d.tokens)).toEqual([null, 150, 50, null]);
  });

  it("skips a workspace's first snapshot rather than inventing a spike", () => {
    // That row is a running total accrued before usage_daily existed for this
    // workspace; attributing it to its day would fake a launch-day spike.
    const rows = [usage("w1", "2026-03-02", 900_000), usage("w1", "2026-03-03", 900_100)];
    expect(dailySpend(rows, DAYS).map((d) => d.tokens)).toEqual([null, null, 100, null]);
  });

  it("contributes nothing from a workspace seen only once", () => {
    expect(dailySpend([usage("w1", "2026-03-02", 500)], DAYS).map((d) => d.tokens)).toEqual([
      null,
      null,
      null,
      null,
    ]);
  });

  it("sums deltas across workspaces on the same day", () => {
    const rows = [
      usage("w1", "2026-03-01", 10),
      usage("w1", "2026-03-02", 40),
      usage("w2", "2026-03-01", 5),
      usage("w2", "2026-03-02", 12),
    ];
    expect(dailySpend(rows, DAYS)[1].tokens).toBe(30 + 7);
  });

  it("keeps an unobserved day null rather than zero", () => {
    // A day the app never folded a session is unknown. A 0 would claim the
    // project was idle, which the data does not say.
    const rows = [usage("w1", "2026-03-01", 10), usage("w1", "2026-03-02", 40)];
    const out = dailySpend(rows, DAYS);
    expect(out[2].tokens).toBeNull();
    expect(out[3].tokens).toBeNull();
  });

  it("distinguishes an observed-but-idle day from an unobserved one", () => {
    // Two snapshots with identical totals: the app looked and nothing moved.
    // That is a real 0, and must not be conflated with the null above.
    const rows = [usage("w1", "2026-03-01", 10), usage("w1", "2026-03-02", 10)];
    expect(dailySpend(rows, DAYS)[1].tokens).toBe(0);
  });

  it("clamps a shrinking total to zero", () => {
    // A re-folded ledger (pruned records) can report less than before; that is
    // a correction, never negative spend.
    const rows = [usage("w1", "2026-03-01", 500), usage("w1", "2026-03-02", 200)];
    expect(dailySpend(rows, DAYS)[1].tokens).toBe(0);
  });

  it("carries cost through the same delta path as tokens", () => {
    const rows = [usage("w1", "2026-03-01", 10, 1.5), usage("w1", "2026-03-02", 40, 4.25)];
    expect(dailySpend(rows, DAYS)[1].cost).toBeCloseTo(2.75);
  });

  it("orders snapshots by day regardless of row order", () => {
    const rows = [
      usage("w1", "2026-03-03", 300),
      usage("w1", "2026-03-01", 100),
      usage("w1", "2026-03-02", 250),
    ];
    expect(dailySpend(rows, DAYS).map((d) => d.tokens)).toEqual([null, 150, 50, null]);
  });
});
