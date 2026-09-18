import { describe, expect, it } from "vitest";
import {
  dayKeysBetween,
  formatClockTime,
  formatCost,
  formatPercent,
  formatTokens,
  recentDays,
} from "./format";

// Local-noon epoch for a YYYY-MM-DD day, so fixtures read as dates and see the
// same local calendar the app runs on.
const at = (day: string, hour = 12): number => {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d, hour).getTime();
};

describe("dayKeysBetween", () => {
  it("lists every day from the first instant's day through the last's", () => {
    expect(dayKeysBetween(at("2026-03-04", 23), at("2026-03-06", 1))).toEqual([
      "2026-03-04",
      "2026-03-05",
      "2026-03-06",
    ]);
  });

  it("collapses a same-day window to one key", () => {
    expect(dayKeysBetween(at("2026-03-05", 9), at("2026-03-05", 17))).toEqual(["2026-03-05"]);
  });
});

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

describe("formatClockTime", () => {
  // Locale decides 24- vs 12-hour, so assert the parts rather than one spelling:
  // the hour and its minutes, to the minute, on the hour boundary.
  it("renders the local hour and minute", () => {
    const text = formatClockTime(new Date(2026, 8, 17, 11, 0).getTime());
    expect(text).toMatch(/\b11[:.]00\b/);
    expect(formatClockTime(new Date(2026, 8, 17, 11, 30).getTime())).toMatch(/\b11[:.]30\b/);
  });
});

describe("formatTokens", () => {
  it("leaves counts under a thousand raw", () => {
    expect(formatTokens(0)).toBe("0");
    expect(formatTokens(999)).toBe("999");
  });

  it("uses k with one decimal only below ten thousand", () => {
    expect(formatTokens(1_000)).toBe("1.0k");
    expect(formatTokens(9_990)).toBe("10.0k");
    expect(formatTokens(12_400)).toBe("12k");
    expect(formatTokens(999_499)).toBe("999k");
  });

  it("uses M between a million and a billion", () => {
    expect(formatTokens(1_000_000)).toBe("1.0M");
    expect(formatTokens(12_340_000)).toBe("12.3M");
    expect(formatTokens(999_900_000)).toBe("999.9M");
  });

  it("uses B at a billion and above", () => {
    expect(formatTokens(1_000_000_000)).toBe("1.0B");
    expect(formatTokens(10_800_000_000)).toBe("10.8B");
    expect(formatTokens(1_234_000_000_000)).toBe("1234.0B");
  });
});

describe("formatPercent", () => {
  it("keeps a decimal only below one percent", () => {
    expect(formatPercent(0)).toBe("0%");
    expect(formatPercent(0.0004)).toBe("<0.1%");
    expect(formatPercent(0.004)).toBe("0.4%");
    expect(formatPercent(0.426)).toBe("43%");
    expect(formatPercent(1)).toBe("100%");
  });
});

describe("formatCost", () => {
  it("keeps sub-cent amounts from rounding to zero", () => {
    expect(formatCost(0)).toBe("$0.000");
    expect(formatCost(0.004)).toBe("<$0.01");
    expect(formatCost(0.034)).toBe("$0.034");
    expect(formatCost(5)).toBe("$5.00");
  });
});
