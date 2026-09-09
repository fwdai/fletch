import { describe, expect, it } from "vitest";
import { normalizeLevel } from "../src/dictation/level";

const fromDb = (db: number) => 10 ** (db / 20);

/** The same mapping as the Mac's `level::normalize`, so the bars read the same
 *  on both ends. */
describe("normalizeLevel", () => {
  it("reads silence and anything under the floor as zero", () => {
    expect(normalizeLevel(0)).toBe(0);
    expect(normalizeLevel(-1)).toBe(0);
    expect(normalizeLevel(fromDb(-50))).toBeCloseTo(0, 5);
    expect(normalizeLevel(fromDb(-70))).toBe(0);
  });

  it("pins at the ceiling", () => {
    expect(normalizeLevel(fromDb(-15))).toBeCloseTo(1, 5);
    expect(normalizeLevel(1)).toBe(1);
  });

  it("is linear in decibels between", () => {
    expect(normalizeLevel(fromDb(-32.5))).toBeCloseTo(0.5, 5);
    expect(normalizeLevel(fromDb(-41.25))).toBeCloseTo(0.25, 5);
  });

  it("never goes down as the input goes up", () => {
    const levels = Array.from({ length: 100 }, (_, i) => normalizeLevel(i / 100));
    for (let i = 1; i < levels.length; i++) expect(levels[i]).toBeGreaterThanOrEqual(levels[i - 1]);
  });
});
