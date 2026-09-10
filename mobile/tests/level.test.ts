import { describe, expect, it } from "vitest";
import { CEIL_DB, FLOOR_DB, normalizeLevel } from "../src/dictation/level";

const fromDb = (db: number) => 10 ** (db / 20);

/** Derived from the range rather than written out, so moving the floor — as
 *  aligning it with the detector's `MIN_RMS` did — can't leave the expectations
 *  behind asserting the old one. `level.rs`'s tests read the constants the same
 *  way. */
const span = CEIL_DB - FLOOR_DB;

/** The same mapping as the Mac's `level::normalize`, so the bars read the same
 *  on both ends. */
describe("normalizeLevel", () => {
  it("reads silence and anything under the floor as zero", () => {
    expect(normalizeLevel(0)).toBe(0);
    expect(normalizeLevel(-1)).toBe(0);
    expect(normalizeLevel(fromDb(FLOOR_DB))).toBeCloseTo(0, 5);
    expect(normalizeLevel(fromDb(FLOOR_DB - 20))).toBe(0);
  });

  it("pins at the ceiling", () => {
    expect(normalizeLevel(fromDb(CEIL_DB))).toBeCloseTo(1, 5);
    expect(normalizeLevel(1)).toBe(1);
  });

  it("is linear in decibels between", () => {
    expect(normalizeLevel(fromDb(FLOOR_DB + span / 2))).toBeCloseTo(0.5, 5);
    expect(normalizeLevel(fromDb(FLOOR_DB + span / 4))).toBeCloseTo(0.25, 5);
  });

  it("never goes down as the input goes up", () => {
    const levels = Array.from({ length: 100 }, (_, i) => normalizeLevel(i / 100));
    for (let i = 1; i < levels.length; i++) expect(levels[i]).toBeGreaterThanOrEqual(levels[i - 1]);
  });
});
