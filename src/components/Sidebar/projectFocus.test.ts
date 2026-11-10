import { describe, expect, it } from "vitest";
import { applyFocus, toggleFocus } from "./projectFocus";

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

describe("applyFocus", () => {
  it("shows everything without a focus", () => {
    expect(keys(applyFocus(groups, [], null))).toEqual(ALL);
  });

  it("narrows to the focused projects, in list order", () => {
    expect(keys(applyFocus(groups, ["c", "a"], null))).toEqual(["a", "c"]);
  });

  it("keeps the project of the open workspace in view", () => {
    expect(keys(applyFocus(groups, ["a"], "c"))).toEqual(["a", "c"]);
  });
});
