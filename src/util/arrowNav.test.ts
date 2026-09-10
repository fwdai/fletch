import { describe, expect, it } from "vitest";
import { arrowTarget } from "./arrowNav";

const items = ["a", "b", "c"];

describe("arrowTarget", () => {
  it("steps down and up without wrapping by default", () => {
    expect(arrowTarget(items, 0, "ArrowDown")).toBe("b");
    expect(arrowTarget(items, 2, "ArrowDown")).toBe("c");
    expect(arrowTarget(items, 1, "ArrowUp")).toBe("a");
    expect(arrowTarget(items, 0, "ArrowUp")).toBe("a");
  });

  it("wraps when asked", () => {
    expect(arrowTarget(items, 2, "ArrowDown", { wrap: true })).toBe("a");
    expect(arrowTarget(items, 0, "ArrowUp", { wrap: true })).toBe("c");
  });

  it("enters the list at the top when focus starts outside it", () => {
    expect(arrowTarget(items, -1, "ArrowDown")).toBe("a");
    expect(arrowTarget(items, -1, "ArrowUp")).toBe("a");
  });

  it("jumps to the ends with Home and End", () => {
    expect(arrowTarget(items, 1, "Home")).toBe("a");
    expect(arrowTarget(items, 1, "End")).toBe("c");
  });

  it("ignores other keys and empty lists", () => {
    expect(arrowTarget(items, 1, "Enter")).toBeUndefined();
    expect(arrowTarget([], 0, "ArrowDown")).toBeUndefined();
  });
});
