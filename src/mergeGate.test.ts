// `mergeGate.ts` is the single classification of GitHub's merge gate that every
// surface (status header, PR card, action bar) renders from. These tests pin the
// one invariant those surfaces trust: `mergeAllowed` is true ONLY when the gate
// positively opens — never inferred from the absence of a conflict, which says
// nothing about CI.

import { describe, expect, it } from "vitest";
import type { Mergeable, MergeState } from "@/api";
import { describeMergeGate } from "@/mergeGate";

// No failing required checks unless a case says otherwise — the neutral context.
const ctx = (mergeable: Mergeable, checksFailed = 0) => ({ checksFailed, mergeable });

describe("describeMergeGate — real merge_state", () => {
  it("opens the gate only on clean and unstable", () => {
    // clean = green light; unstable = only non-required checks fail, still
    // mergeable. Everything else is a closed gate.
    expect(describeMergeGate("clean", ctx("mergeable"))).toMatchObject({
      situation: "ready",
      mergeAllowed: true,
    });
    expect(describeMergeGate("unstable", ctx("mergeable"))).toMatchObject({
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
