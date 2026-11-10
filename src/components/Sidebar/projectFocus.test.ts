import { describe, expect, it } from "vitest";
import { applyFocus, normalizeFocus, toggleFocus } from "./projectFocus";

const ALL = ["a", "b", "c"];
const groups = ALL.map((key) => ({ key }));
const keys = (gs: { key: string }[]) => gs.map((g) => g.key);

describe("toggleFocus", () => {
  it("adds a project to an empty focus", () => {
    expect(toggleFocus([], "b", ALL)).toEqual(["b"]);
  });

  it("keeps the list order whatever order projects were ticked in", () => {
    expect(toggleFocus(["c"], "a", ALL)).toEqual(["a", "c"]);
  });

  it("takes a ticked project out", () => {
    expect(toggleFocus(["a", "c"], "a", ALL)).toEqual(["c"]);
  });

  it("is all projects again once the last one is unticked", () => {
    expect(toggleFocus(["b"], "b", ALL)).toEqual([]);
  });

  it("is all projects again once every one is ticked", () => {
    expect(toggleFocus(["a", "b"], "c", ALL)).toEqual([]);
  });

  it("drops keys of projects that no longer exist", () => {
    expect(toggleFocus(["gone", "a"], "b", ALL)).toEqual(["a", "b"]);
  });
});

describe("normalizeFocus", () => {
  it("orders by the list, drops what is gone, and reads every project as no focus", () => {
    expect(normalizeFocus(["c", "gone", "a"], ALL)).toEqual(["a", "c"]);
    expect(normalizeFocus(["c", "b", "a"], ALL)).toEqual([]);
  });
});

describe("applyFocus", () => {
  it("shows everything without a focus", () => {
    expect(keys(applyFocus(groups, []))).toEqual(ALL);
  });

  it("narrows to the focused projects, in list order", () => {
    expect(keys(applyFocus(groups, ["c", "a"]))).toEqual(["a", "c"]);
  });
});
