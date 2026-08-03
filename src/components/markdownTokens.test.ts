import { describe, expect, it } from "vitest";
import { splitTokens, tokenPattern } from "./markdownTokens";

const pattern = (...tokens: string[]) => {
  const p = tokenPattern(new Set(tokens));
  if (!p) throw new Error("expected a pattern");
  return p;
};

describe("tokenPattern", () => {
  it("is null with nothing to match", () => {
    expect(tokenPattern(new Set())).toBeNull();
  });

  it("drops tokens `\\b` can't delimit rather than matching them loosely", () => {
    expect(tokenPattern(new Set(["-FLT-1", "FLT-1-", "#FLT"]))).toBeNull();
  });

  it("prefers the longest match — alternation is leftmost, not longest", () => {
    const p = pattern("FLT-10", "FLT-104");
    expect(splitTokens("see FLT-104", p)).toEqual([{ text: "see " }, { token: "FLT-104" }]);
  });
});

describe("splitTokens", () => {
  it("returns nothing when no token appears, so the caller keeps its node", () => {
    expect(splitTokens("nothing to see here", pattern("FLT-104"))).toEqual([]);
  });

  it("splits prose around every occurrence", () => {
    const p = pattern("FLT-104", "MCA-7");
    expect(splitTokens("FLT-104 blocks MCA-7 today", p)).toEqual([
      { token: "FLT-104" },
      { text: " blocks " },
      { token: "MCA-7" },
      { text: " today" },
    ]);
  });

  it("matches only codes on the board — a plausible shape is not enough", () => {
    expect(splitTokens("FLT-999 is not ours", pattern("FLT-104"))).toEqual([]);
  });

  it("respects word boundaries at both ends", () => {
    const p = pattern("FLT-104");
    // A longer number is a different code, and a word-glued match is not one.
    expect(splitTokens("FLT-1042 and xFLT-104", p)).toEqual([]);
    // Punctuation around it is fine — that's how a sentence quotes a code.
    expect(splitTokens("(FLT-104).", p)).toEqual([
      { text: "(" },
      { token: "FLT-104" },
      { text: ")." },
    ]);
  });

  it("is case-sensitive: the set is the authority, not the shape", () => {
    expect(splitTokens("flt-104", pattern("FLT-104"))).toEqual([]);
  });

  it("is re-runnable — a shared pattern carries no lastIndex between texts", () => {
    const p = pattern("FLT-104");
    expect(splitTokens("FLT-104", p)).toEqual([{ token: "FLT-104" }]);
    expect(splitTokens("FLT-104", p)).toEqual([{ token: "FLT-104" }]);
  });
});
