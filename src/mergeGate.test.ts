// `mergeGate.ts` is the single classification of GitHub's merge gate that every
// surface (status header, PR card, action bar) renders from. These tests pin the
// one invariant those surfaces trust: `mergeAllowed` is true ONLY when the gate
// positively opens — never inferred from the absence of a conflict, which says
// nothing about CI.

import { describe, expect, it } from "vitest";
import type { Mergeable, MergeState } from "@/api";
import { describeMergeGate, type MergeGateSituation, mergeGateLabel } from "@/mergeGate";

// No failing required checks unless a case says otherwise — the neutral context.
const ctx = (mergeable: Mergeable, checksFailed = 0) => ({ checksFailed, mergeable });

describe("describeMergeGate — real merge_state", () => {
  it("opens the gate only on clean and unstable", () => {
    // clean = green light; unstable = mergeable anyway. Everything else is a
    // closed gate.
    expect(describeMergeGate("clean", ctx("mergeable"))).toMatchObject({
      situation: "ready",
      mergeAllowed: true,
    });
    expect(describeMergeGate("unstable", ctx("mergeable"))).toMatchObject({
      situation: "mergeable-soft",
      mergeAllowed: true,
    });
  });

  it("calls a failing check failing even when the gate stays open", () => {
    // The regression this pins. A repo with no REQUIRED status checks — GitHub's
    // default — reports a failing run as `unstable`, not `blocked`. Classifying
    // that as "only optional checks failing" left a red PR with no surface
    // offering to fix it and autopilot concluding it had nothing to do.
    expect(describeMergeGate("unstable", ctx("mergeable", 1))).toEqual({
      situation: "checks-failing",
      tone: "attention",
      // Still true, and deliberately: GitHub really would take this merge. The
      // situation names the problem; this names the forge's verdict.
      mergeAllowed: true,
      needsUpdate: false,
    });
    // Nothing failing → the arm's only remaining meaning, checks still running.
    expect(describeMergeGate("unstable", ctx("mergeable", 0))).toMatchObject({
      situation: "mergeable-soft",
      mergeAllowed: true,
    });
  });

  it("keeps the gate closed for every blocking state", () => {
    // `blocked` splits on failing required checks (agent-fixable) vs. a pure
    // review gate, but both forbid merging.
    expect(describeMergeGate("blocked", ctx("mergeable", 2))).toMatchObject({
      situation: "checks-failing",
      mergeAllowed: false,
    });
    expect(describeMergeGate("blocked", ctx("mergeable", 0))).toMatchObject({
      situation: "review-required",
      mergeAllowed: false,
    });
    expect(describeMergeGate("behind", ctx("mergeable"))).toMatchObject({
      situation: "behind",
      mergeAllowed: false,
      needsUpdate: true,
    });
    expect(describeMergeGate("dirty", ctx("mergeable"))).toMatchObject({
      situation: "conflicts",
      mergeAllowed: false,
      needsUpdate: true,
    });
    expect(describeMergeGate("draft", ctx("mergeable"))).toMatchObject({
      situation: "draft",
      mergeAllowed: false,
    });
  });

  it("stays computing while GitHub is still resolving the gate", () => {
    for (const s of ["unknown", "has_hooks"] as const) {
      expect(describeMergeGate(s, ctx("mergeable"))).toMatchObject({
        situation: "computing",
        mergeAllowed: false,
      });
    }
  });
});

describe("describeMergeGate — no checks data (mergeable-only fallback)", () => {
  it("never claims merge-ready off a `mergeable` verdict with no CI knowledge", () => {
    // The bug this pins: `merge_state === null` means zero check knowledge, so
    // "no conflict" is NOT "safe to merge" — required checks could be failing or
    // unrun. The surfaces still get the honest `no-conflicts` situation, but the
    // gate stays shut.
    const gate = describeMergeGate(null, ctx("mergeable"));
    expect(gate).toEqual({
      situation: "no-conflicts",
      tone: "info",
      mergeAllowed: false,
      needsUpdate: false,
    });
  });

  it("still reports a real conflict when GitHub actually reports one", () => {
    expect(describeMergeGate(null, ctx("conflicting"))).toEqual({
      situation: "conflicts",
      tone: "attention",
      mergeAllowed: false,
      needsUpdate: true,
    });
  });

  it("treats a not-yet-computed verdict as still computing, not as a conflict", () => {
    // `unknown` is GitHub's "haven't computed it yet" (and every DB snapshot's
    // value) — never a false "can't merge".
    expect(describeMergeGate(null, ctx("unknown"))).toMatchObject({
      situation: "computing",
      mergeAllowed: false,
    });
  });

  it("resolves the asymmetry: no `mergeable`-only path over-claims merge-readiness", () => {
    // `unknown` merge_state, `null` + `mergeable: "unknown"`, and now `null` +
    // `mergeable: "mergeable"` all agree: absent check data → `mergeAllowed`
    // false. Only a positively-open real gate (clean/unstable) may open it.
    const conservative: Array<[MergeState | null, Mergeable]> = [
      ["unknown", "mergeable"],
      [null, "unknown"],
      [null, "mergeable"],
    ];
    for (const [state, mergeable] of conservative) {
      expect(describeMergeGate(state, ctx(mergeable)).mergeAllowed).toBe(false);
    }
  });
});

// The terse phrasing moved here out of StatusHeader when the roadmap card became
// its second consumer. These pin the two things a shared label owes both: every
// situation says something (a missing arm is a chip rendering "undefined"), and
// the branch-relative ones name a real branch — or the honest generic word for a
// caller with no checkout to read one from.
describe("mergeGateLabel", () => {
  it("has a phrase for every situation", () => {
    const situations: MergeGateSituation[] = [
      "ready",
      "mergeable-soft",
      "checks-failing",
      "review-required",
      "behind",
      "conflicts",
      "draft",
      "computing",
      "no-conflicts",
    ];
    for (const s of situations) {
      const label = mergeGateLabel(s, "main");
      expect(label, s).toBeTruthy();
      expect(label, s).not.toContain("undefined");
    }
  });

  it("names the base branch on the two situations that are about it", () => {
    expect(mergeGateLabel("behind", "main")).toBe("behind main");
    expect(mergeGateLabel("conflicts", "develop")).toBe("conflicts with develop");
  });

  it("says what `mergeable-soft` now means, which is not 'optional failures'", () => {
    expect(mergeGateLabel("mergeable-soft")).toBe("checks still running");
    expect(mergeGateLabel("checks-failing")).toBe("checks failing");
  });

  it("falls back to the generic word when the caller has no base branch", () => {
    expect(mergeGateLabel("behind")).toBe("behind base");
    expect(mergeGateLabel("conflicts")).toBe("conflicts with base");
    // Everything else ignores the argument entirely.
    expect(mergeGateLabel("ready")).toBe("ready to merge");
  });
});
