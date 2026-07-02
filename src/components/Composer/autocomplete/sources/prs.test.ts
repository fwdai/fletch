import { describe, expect, it } from "vitest";
import type { PrSummary } from "@/api";
import { filterPrs } from "./prs";

const pr = (number: number, title = `PR ${number}`): PrSummary => ({
  number,
  title,
  state: "open",
});

describe("filterPrs", () => {
  const prs = [pr(7), pr(42), pr(120), pr(123), pr(99)];

  it("lists most-recent (highest number) first for an empty query", () => {
    expect(filterPrs(prs, "").map((p) => p.number)).toEqual([123, 120, 99, 42, 7]);
  });

  it("keeps only PRs whose number contains the digits", () => {
    expect(filterPrs(prs, "12").map((p) => p.number)).toEqual([123, 120]);
  });

  it("ranks prefix matches ahead of mid-number matches", () => {
    // "2" appears in 42, 120, 123 — those starting with "2" would rank first;
    // here none start with "2", so they fall back to recency order.
    expect(filterPrs([pr(2), pr(120), pr(42)], "2").map((p) => p.number)).toEqual([2, 120, 42]);
  });

  it("honors the limit", () => {
    expect(filterPrs(prs, "", 2)).toHaveLength(2);
  });
});
