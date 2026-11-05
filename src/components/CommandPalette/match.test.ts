import { describe, expect, it } from "vitest";
import { bestMatch, fuzzyMatch, highlightRuns } from "./match";

const score = (q: string, t: string) => fuzzyMatch(q, t)?.score ?? null;

describe("fuzzyMatch", () => {
  it("matches everything on an empty query, at zero, so the list keeps its order", () => {
    expect(fuzzyMatch("", "New agent")).toEqual({ score: 0, indices: [] });
    expect(fuzzyMatch("   ", "New agent")).toEqual({ score: 0, indices: [] });
  });

  it("ranks exact, then prefix, then word start, then substring, then subsequence", () => {
    const exact = score("git panel", "Git panel");
    const prefix = score("git", "Git panel");
    const word = score("panel", "Git panel");
    const substring = score("ane", "Git panel");
    const loose = score("gpl", "Git panel");
    expect(exact).toBeGreaterThan(prefix as number);
    expect(prefix).toBeGreaterThan(word as number);
    expect(word).toBeGreaterThan(substring as number);
    expect(substring).toBeGreaterThan(loose as number);
    expect(loose).toBeGreaterThan(0);
  });

  it("returns null when a query character is missing", () => {
    expect(fuzzyMatch("xyz", "Git panel")).toBeNull();
    expect(fuzzyMatch("panelz", "Git panel")).toBeNull();
  });

  it("is case-insensitive and reports where the query landed", () => {
    expect(fuzzyMatch("PANEL", "Git panel")).toEqual({
      score: expect.any(Number),
      indices: [4, 5, 6, 7, 8],
    });
    expect(fuzzyMatch("gp", "Git panel")?.indices).toEqual([0, 4]);
  });

  it("prefers a shorter candidate at the same tier", () => {
    expect(score("new", "New agent")).toBeGreaterThan(
      score("new", "New agent in another project") as number,
    );
  });

  it("prefers a word-start substring over one buried mid-word", () => {
    expect(score("run", "Run panel")).toBeGreaterThan(score("run", "Prune branches") as number);
  });

  it("walks a loose subsequence across words, rewarding word starts and runs", () => {
    const m = fuzzyMatch("nag", "New agent");
    expect(m).not.toBeNull();
    expect(m?.indices).toEqual([0, 4, 5]);
  });
});

describe("bestMatch", () => {
  it("falls back to the keywords, below any literal match on a label", () => {
    const onKeyword = bestMatch("ship", "Commit & open a pull request", "github  ship  publish");
    expect(onKeyword).not.toBeNull();
    expect(onKeyword?.indices).toEqual([]);
    // Even an exact keyword loses to a label the query is merely inside of.
    expect(onKeyword?.score).toBeLessThan(fuzzyMatch("ship", "Friendship bracelet")?.score ?? 0);
  });

  it("keeps the label's highlight when the label is the better match", () => {
    const m = bestMatch("git", "Git panel", "commit  push  git");
    expect(m?.indices).toEqual([0, 1, 2]);
  });

  it("returns null when neither label nor keywords match", () => {
    expect(bestMatch("zzz", "Git panel", "commit  push")).toBeNull();
  });
});

describe("highlightRuns", () => {
  it("splits the text into alternating plain and highlighted runs", () => {
    expect(highlightRuns("Git panel", [0, 1, 2])).toEqual([
      ["Git", true],
      [" panel", false],
    ]);
    expect(highlightRuns("Git panel", [0, 4])).toEqual([
      ["G", true],
      ["it ", false],
      ["p", true],
      ["anel", false],
    ]);
  });

  it("returns the whole text as one plain run with nothing highlighted", () => {
    expect(highlightRuns("Git panel", [])).toEqual([["Git panel", false]]);
  });
});
