import { describe, expect, it } from "vitest";
import type { PrChecks, PrSetEntry, PrState, TrackedRepo } from "@/api";
import { checkoutPrs, prSnapshot, prTint, summarizePrSet, worstOpenPr } from "@/util/prState";

const pr = (number: number, overrides: Partial<PrState> = {}): PrState => ({
  number,
  url: `https://github.com/o/r/pull/${number}`,
  state: "open",
  title: `PR ${number}`,
  mergeable: "mergeable",
  ...overrides,
});
const checks = (rollup: PrChecks["rollup"]): PrChecks => ({
  merge_state: "clean",
  rollup,
  total: 1,
  passed: rollup === "passing" ? 1 : 0,
  failed: rollup === "failing" ? 1 : 0,
  pending: rollup === "pending" ? 1 : 0,
  required_failing: [],
  runs: [],
});
const entry = (
  number: number,
  rollup: PrChecks["rollup"] | null,
  overrides: Partial<PrState> = {},
): PrSetEntry => ({ state: pr(number, overrides), checks: rollup ? checks(rollup) : null });

describe("prTint", () => {
  it("names settled PRs by state, whatever their stale checks say", () => {
    expect(prTint(pr(1, { state: "merged" }), checks("failing"))).toEqual({
      variant: "pr-merged",
      word: "merged",
    });
    expect(prTint(pr(1, { state: "closed" }), null)).toEqual({
      variant: "pr-closed",
      word: "closed",
    });
  });

  it("tints an open PR failing > conflicts > pending > passing", () => {
    expect(prTint(pr(1, { mergeable: "conflicting" }), checks("failing")).variant).toBe("pr-fail");
    expect(prTint(pr(1, { mergeable: "conflicting" }), checks("passing"))).toEqual({
      variant: "warn",
      word: "conflicts",
    });
    expect(prTint(pr(1), checks("pending"))).toEqual({
      variant: "pr-open",
      word: "checks running",
    });
    expect(prTint(pr(1), null)).toEqual({ variant: "pr-open", word: "open" });
    expect(prTint(pr(1), checks("passing")).variant).toBe("pr-pass");
  });
});

describe("summarizePrSet", () => {
  it("labels one PR by number and several by count, itemizing the tip", () => {
    expect(summarizePrSet([entry(4, "passing")])).toEqual({
      variant: "pr-pass",
      icon: "pr",
      label: "#4",
      tip: "#4 checks passing",
    });
    const several = summarizePrSet([entry(9, "passing"), entry(8, null, { state: "merged" })]);
    expect(several.label).toBe("2 PRs");
    expect(several.tip).toBe("#9 checks passing · #8 merged");
  });

  it("lets the worst open PR colour the pill", () => {
    const passing = entry(3, "passing");
    expect(summarizePrSet([passing, entry(2, "failing")]).variant).toBe("pr-fail");
    expect(
      summarizePrSet([passing, entry(2, "passing", { mergeable: "conflicting" })]).variant,
    ).toBe("warn");
    expect(summarizePrSet([passing, entry(2, null)]).variant).toBe("pr-open");
    expect(summarizePrSet([passing, entry(2, "passing")]).variant).toBe("pr-pass");
  });

  it("ignores settled PRs' checks while any PR is open", () => {
    const set = [entry(3, "passing"), entry(2, "failing", { state: "merged" })];
    expect(summarizePrSet(set).variant).toBe("pr-pass");
  });

  it("falls back to closed grey over merged purple when nothing is open", () => {
    const merged = entry(2, null, { state: "merged" });
    expect(summarizePrSet([merged, entry(1, null, { state: "closed" })])).toMatchObject({
      variant: "pr-closed",
      icon: "pr",
    });
    expect(summarizePrSet([merged])).toMatchObject({ variant: "pr-merged", icon: "merge" });
  });
});

describe("worstOpenPr", () => {
  it("picks the worst open PR, first on a tie, null when none is open", () => {
    const a = entry(3, "pending");
    const b = entry(2, null);
    expect(worstOpenPr([a, b, entry(1, "failing", { state: "closed" })])).toBe(a);
    expect(worstOpenPr([a, entry(2, "failing")])?.state.number).toBe(2);
    expect(worstOpenPr([entry(1, null, { state: "merged" })])).toBeNull();
  });
});

describe("checkoutPrs", () => {
  it("is the focused PR alone when no set is known", () => {
    expect(checkoutPrs(undefined, pr(5), checks("passing"))).toEqual([entry(5, "passing")]);
    expect(checkoutPrs(undefined, null, null)).toEqual([]);
  });

  it("refreshes the set's focused entry from the focused maps", () => {
    const set = [entry(6, "failing"), entry(5, "pending")];
    const focused = pr(5, { title: "renamed" });
    expect(checkoutPrs(set, focused, checks("passing"))).toEqual([
      entry(6, "failing"),
      { state: focused, checks: checks("passing") },
    ]);
    // No focused checks yet: the set's own copy stands.
    expect(checkoutPrs(set, pr(5), null)[1].checks).toEqual(checks("pending"));
  });

  it("adds a focused PR the set has not caught up with, newest first", () => {
    expect(checkoutPrs([entry(5, null)], pr(7), null).map((e) => e.state.number)).toEqual([7, 5]);
  });
});

const repo = (overrides: Partial<TrackedRepo> = {}): TrackedRepo => ({
  repo_path: "/r",
  subdir: "repo",
  branch: "feat/x",
  parent_branch: "main",
  pr_number: 42,
  pr_url: "https://github.com/o/r/pull/42",
  pr_title: "feat: x",
  pr_state: "merged",
  ...overrides,
});

describe("prSnapshot", () => {
  it("rebuilds the persisted PR state from the repo record", () => {
    expect(prSnapshot(repo())).toEqual({
      number: 42,
      url: "https://github.com/o/r/pull/42",
      state: "merged",
      title: "feat: x",
      mergeable: "unknown",
    });
  });

  it("is null without a bound PR number or persisted state", () => {
    expect(prSnapshot(undefined)).toBeNull();
    expect(prSnapshot(repo({ pr_number: null }))).toBeNull();
    expect(prSnapshot(repo({ pr_state: null }))).toBeNull();
  });

  it("rejects an unknown state string rather than fabricating a badge", () => {
    expect(prSnapshot(repo({ pr_state: "weird" }))).toBeNull();
  });

  it("degrades missing url/title to empty strings", () => {
    const pr = prSnapshot(repo({ pr_url: null, pr_title: null }));
    expect(pr).toMatchObject({ number: 42, url: "", title: "" });
  });
});
