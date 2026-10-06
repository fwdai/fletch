import { describe, expect, it } from "vitest";
import type { PrChecks, PrSetEntry, PrState } from "@/api";
import { checkoutPrs, prTint, summarizePrSet } from "@/util/prSummary";

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

  it("reads a conflict from either the PR or its checks' merge state", () => {
    // The title bar's badge goes by `merge_state`; the sidebar must agree.
    const dirty = { ...checks("passing"), merge_state: "dirty" as const };
    expect(prTint(pr(1), dirty).variant).toBe("warn");
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

describe("checkoutPrs", () => {
  it("is the focused PR alone when no set is known", () => {
    expect(checkoutPrs(undefined, pr(5), checks("passing"))).toEqual([entry(5, "passing")]);
    expect(checkoutPrs(undefined, null, null)).toEqual([]);
  });

  it("is the set, focused PR first and the rest in set order", () => {
    const set = [entry(7, null), entry(6, "failing"), entry(5, "pending")];
    expect(checkoutPrs(set, pr(6), null).map((e) => e.state.number)).toEqual([6, 7, 5]);
    // The set's copy stands: the focused reads already upserted it.
    expect(checkoutPrs(set, pr(6), checks("passing"))[0]).toBe(set[1]);
    expect(checkoutPrs(set, null, null).map((e) => e.state.number)).toEqual([7, 6, 5]);
  });
});
