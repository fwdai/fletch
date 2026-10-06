import { describe, expect, it } from "vitest";
import type { PrSetEntry, PrStatus } from "@/api";
import { splitChips } from "./chips";

const entry = (number: number, state: PrStatus = "open"): PrSetEntry => ({
  state: { number, url: "", state, title: "", mergeable: "unknown" },
  checks: null,
});

const numbers = (entries: PrSetEntry[]) => entries.map((e) => e.state.number);

describe("splitChips", () => {
  it("shows every PR while they fit", () => {
    const set = [entry(4), entry(3, "merged"), entry(2), entry(1, "closed")];

    expect(splitChips(set, 2)).toEqual({ inline: set, overflow: [] });
  });

  it("keeps open PRs inline before settled ones, in the set's order", () => {
    const set = [
      entry(9, "merged"),
      entry(8),
      entry(7, "closed"),
      entry(6),
      entry(5),
      entry(4),
      entry(3),
    ];

    const { inline, overflow } = splitChips(set, 8);

    expect(numbers(inline)).toEqual([8, 6, 5, 4]);
    expect(numbers(overflow)).toEqual([9, 7, 3]);
  });

  it("always shows the focused PR, even a settled one from deep in the set", () => {
    const set = [entry(9), entry(8), entry(7), entry(6), entry(5), entry(1, "merged")];

    const { inline, overflow } = splitChips(set, 1);

    expect(numbers(inline)).toEqual([9, 8, 7, 1]);
    expect(numbers(overflow)).toEqual([6, 5]);
  });

  it("falls back to newest first when nothing is focused", () => {
    const set = [entry(5, "merged"), entry(4, "merged"), entry(3), entry(2), entry(1)];

    expect(numbers(splitChips(set, null).inline)).toEqual([5, 3, 2, 1]);
  });
});
