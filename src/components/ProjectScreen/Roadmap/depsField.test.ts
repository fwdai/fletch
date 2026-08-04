import { describe, expect, it } from "vitest";
import { addDep, removeDep, resolveCode, suggestCodes } from "./depsField";

const board = new Set(["FLT-100", "FLT-101", "FLT-142"]);

describe("resolveCode", () => {
  it("matches exactly first, then case-insensitively", () => {
    expect(resolveCode("  FLT-100 ", board)).toBe("FLT-100");
    expect(resolveCode("flt-142", board)).toBe("FLT-142");
    // Nothing on the board: handed back as typed, for the refusal to name.
    expect(resolveCode("nope", board)).toBe("nope");
  });
});

describe("addDep", () => {
  it("adds a code that is on the board", () => {
    expect(addDep([], "flt-100", board)).toEqual({ deps: ["FLT-100"], error: null });
    expect(addDep(["FLT-100"], "FLT-101", board)).toEqual({
      deps: ["FLT-100", "FLT-101"],
      error: null,
    });
  });

  it("does nothing for an empty box", () => {
    expect(addDep(["FLT-100"], "   ", board)).toEqual({ deps: null, error: null });
  });

  it("keeps a duplicate out without calling it an error", () => {
    expect(addDep(["FLT-100"], "FLT-100", board)).toEqual({ deps: ["FLT-100"], error: null });
  });

  it("refuses a code that isn't on the board", () => {
    const { deps, error } = addDep([], "FLT-999", board);
    expect(deps).toBeNull();
    expect(error).toContain("FLT-999");
  });

  it("refuses the item's own code", () => {
    // The loop of length one. Longer loops are the backend's answer — it holds
    // the graph — and arrive as the dialog's error.
    const { deps, error } = addDep([], "flt-142", board, "FLT-142");
    expect(deps).toBeNull();
    expect(error).toContain("itself");
  });
});

describe("removeDep", () => {
  it("drops just that chip", () => {
    expect(removeDep(["FLT-100", "FLT-101"], "FLT-100")).toEqual(["FLT-101"]);
  });
});

describe("suggestCodes", () => {
  it("offers the board, minus this item and what is already chosen", () => {
    expect(suggestCodes("", board, ["FLT-101"], "FLT-142")).toEqual(["FLT-100"]);
  });

  it("matches on the number alone, since the prefix is the same for every row", () => {
    expect(suggestCodes("10", board, [])).toEqual(["FLT-100", "FLT-101"]);
    expect(suggestCodes("142", board, [])).toEqual(["FLT-142"]);
    expect(suggestCodes("zzz", board, [])).toEqual([]);
  });

  it("caps the list so it stays glanceable", () => {
    const many = new Set(Array.from({ length: 30 }, (_, n) => `FLT-${100 + n}`));
    expect(suggestCodes("", many, [], null, 3)).toEqual(["FLT-100", "FLT-101", "FLT-102"]);
  });
});
