// The agent row's PR pill (src/lib/shipPill.ts): one line off the shared
// merge-gate classification, so the row agrees with the Ship tab behind it.

import type { PrChecks, PrState } from "@desktop/api/types/pr";
import { describe, expect, it } from "vitest";
import { prPill } from "../src/lib/shipPill";

const pr = (over: Partial<PrState> = {}): PrState => ({
  number: 642,
  url: "https://github.com/o/r/pull/642",
  state: "open",
  title: "t",
  mergeable: "unknown",
  ...over,
});

const checks = (over: Partial<PrChecks> = {}): PrChecks => ({
  merge_state: "clean",
  rollup: "passing",
  total: 1,
  passed: 1,
  failed: 0,
  pending: 0,
  required_failing: [],
  runs: [],
  ...over,
});

describe("prPill", () => {
  it("is nothing without a PR", () => {
    expect(prPill(null, null)).toBeNull();
    expect(prPill(undefined, checks())).toBeNull();
  });

  it("names merged and closed PRs without consulting the gate", () => {
    expect(prPill(pr({ state: "merged" }), checks({ merge_state: "dirty" }))).toEqual({
      text: "merged",
      tone: "merged",
    });
    expect(prPill(pr({ state: "closed" }), null)).toEqual({ text: "#642 closed", tone: "" });
  });

  it("reads an open PR's gate off the checks, in the gate's own words", () => {
    expect(prPill(pr(), checks())).toEqual({ text: "#642 · ready to merge", tone: "ok" });
    expect(prPill(pr(), checks({ merge_state: "blocked", required_failing: ["build"] }))).toEqual({
      text: "#642 · checks failing",
      tone: "warn",
    });
    expect(prPill(pr(), checks({ merge_state: "blocked" }))).toEqual({
      text: "#642 · review required",
      tone: "warn",
    });
    expect(prPill(pr(), checks({ merge_state: "dirty" }))).toEqual({
      text: "#642 · conflicts with base",
      tone: "warn",
    });
    // The soft and informational gates are said but not tinted.
    expect(prPill(pr(), checks({ merge_state: "unstable" }))).toEqual({
      text: "#642 · checks still running",
      tone: "",
    });
    expect(prPill(pr(), checks({ merge_state: "draft" }))).toEqual({
      text: "#642 · draft",
      tone: "",
    });
  });

  it("falls back to GitHub's coarse mergeable verdict before any checks arrive", () => {
    expect(prPill(pr({ mergeable: "unknown" }), null)).toEqual({
      text: "#642 · checking…",
      tone: "",
    });
    expect(prPill(pr({ mergeable: "mergeable" }), undefined)).toEqual({
      text: "#642 · no conflicts",
      tone: "",
    });
    expect(prPill(pr({ mergeable: "conflicting" }), null)).toEqual({
      text: "#642 · conflicts with base",
      tone: "warn",
    });
  });
});
