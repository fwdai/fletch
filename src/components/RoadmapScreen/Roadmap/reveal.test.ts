import { describe, expect, it } from "vitest";
import { revealRefusal, revealTarget } from "./reveal";

const rows = [
  { code: "FLT-100", status: "open" },
  { code: "FLT-101", status: "proposed" },
  { code: "FLT-102", status: "active" },
  { code: "FLT-103", status: "done" },
];

describe("revealTarget", () => {
  it("sends every row the board draws to the board", () => {
    for (const code of ["FLT-100", "FLT-101", "FLT-102"]) {
      expect(revealTarget(code, rows)).toEqual({ kind: "board", code });
    }
  });

  // The defect this exists for: the board renders `status !== done`, so a chip
  // for a shipped item used to focus nothing at all — and the standup digest's
  // subject *is* shipped items.
  it("sends a shipped row to where it lives instead of to the board", () => {
    expect(revealTarget("FLT-103", rows)).toEqual({ kind: "shipped", code: "FLT-103" });
  });

  it("refuses a code this board has never held", () => {
    expect(revealTarget("FLT-999", rows)).toEqual({ kind: "unknown", code: "FLT-999" });
    expect(revealTarget("OTH-100", rows)).toEqual({ kind: "unknown", code: "OTH-100" });
    expect(revealTarget("FLT-100", [])).toEqual({ kind: "unknown", code: "FLT-100" });
  });
});

describe("revealRefusal", () => {
  it("only speaks up when there is nowhere to go", () => {
    expect(revealRefusal({ kind: "board", code: "FLT-100" })).toBeNull();
    expect(revealRefusal({ kind: "shipped", code: "FLT-103" })).toBeNull();
    expect(revealRefusal({ kind: "unknown", code: "FLT-999" })).toContain("FLT-999");
  });
});
