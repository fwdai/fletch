import { describe, expect, it } from "vitest";
import { localDay, parseNudge, visit } from "./nudge";

describe("feedback nudge schedule", () => {
  it("records the first day and stays quiet", () => {
    const r = visit({ days: [], shown: false }, "2026-10-01");
    expect(r.show).toBe(false);
    expect(r.next).toEqual({ days: ["2026-10-01"], shown: false });
  });

  it("does not count a second open on the same day", () => {
    const prev = { days: ["2026-10-01"], shown: false };
    const r = visit(prev, "2026-10-01");
    expect(r.show).toBe(false);
    expect(r.next).toBe(prev);
  });

  it("shows on the third distinct day, however far apart", () => {
    const d1 = visit({ days: [], shown: false }, "2026-10-01");
    const d2 = visit(d1.next, "2026-10-14");
    expect(d2.show).toBe(false);
    const d3 = visit(d2.next, "2026-12-25");
    expect(d3.show).toBe(true);
    expect(d3.next.shown).toBe(true);
  });

  it("never shows again once shown", () => {
    const shown = { days: ["2026-10-01", "2026-10-02", "2026-10-03"], shown: true };
    expect(visit(shown, "2026-10-04").show).toBe(false);
    expect(visit(shown, "2026-10-04").next).toBe(shown);
  });

  it("tolerates a missing or broken row", () => {
    expect(parseNudge(null)).toEqual({ days: [], shown: false });
    expect(parseNudge("not json")).toEqual({ days: [], shown: false });
    expect(parseNudge('{"days":"x","shown":"yes"}')).toEqual({ days: [], shown: false });
  });

  it("round-trips through JSON", () => {
    const n = { days: ["2026-10-01", "2026-10-02"], shown: false };
    expect(parseNudge(JSON.stringify(n))).toEqual(n);
  });

  it("formats the local day with zero padding", () => {
    expect(localDay(new Date(2026, 0, 5))).toBe("2026-01-05");
  });
});
