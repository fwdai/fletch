import { describe, expect, it } from "vitest";
import type { WfPausedReason } from "../../api";
import { pausedLabel } from "./status";

/** Every reason the engine can pause on (spec §6.2). Listed literally rather
 *  than derived, so adding a variant to the union fails this test until the
 *  label exists — a surface that says "Paused — undefined" is worse than one
 *  that doesn't compile. */
const REASONS: WfPausedReason[] = [
  "approval",
  "question",
  "blocked_gate",
  "budget_exceeded",
  "conflict",
  "stalled",
];

describe("pausedLabel", () => {
  it("names every pause reason in plain language", () => {
    expect(REASONS.map(pausedLabel)).toEqual([
      "needs approval",
      "awaiting answer",
      "gate not met",
      "budget reached",
      "merge conflict",
      "stalled",
    ]);
  });

  it("never yields an empty or shouty label — these read inline in a sentence", () => {
    for (const r of REASONS) {
      const label = pausedLabel(r);
      expect(label.length).toBeGreaterThan(0);
      expect(label).toBe(label.toLowerCase());
    }
  });
});
