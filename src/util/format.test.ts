import { describe, expect, it } from "vitest";
import { formatCost, formatPercent, formatTokens } from "./format";

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
