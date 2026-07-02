import { describe, expect, it } from "vitest";
import { lineDiffCounts } from "@/util/lineDiff";

describe("lineDiffCounts", () => {
  it("counts a brand-new body as pure additions", () => {
    expect(lineDiffCounts("", "a\nb\nc")).toEqual({ additions: 3, deletions: 0 });
  });

  it("counts a full removal as pure deletions", () => {
    expect(lineDiffCounts("a\nb", "")).toEqual({ additions: 0, deletions: 2 });
  });

  it("ignores unchanged lines via the LCS", () => {
    // keep a + c, drop b, replace nothing else -> -1
    expect(lineDiffCounts("a\nb\nc", "a\nc")).toEqual({ additions: 0, deletions: 1 });
  });

  it("only counts genuinely changed lines when replacing a block", () => {
    // shared: x, z (2). old has 3 lines, new has 4 -> +2 -1
    expect(lineDiffCounts("x\nOLD\nz", "x\nNEW1\nNEW2\nz")).toEqual({
      additions: 2,
      deletions: 1,
    });
  });

  it("treats a trailing newline as no extra line", () => {
    expect(lineDiffCounts("a\n", "a\nb\n")).toEqual({ additions: 1, deletions: 0 });
  });

  it("reports no change for identical text", () => {
    expect(lineDiffCounts("a\nb\n", "a\nb\n")).toEqual({ additions: 0, deletions: 0 });
  });
});
