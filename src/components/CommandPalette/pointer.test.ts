import { describe, expect, it } from "vitest";
import { type Point, pointerMoved } from "./pointer";

describe("pointerMoved", () => {
  it("does not count the first sample as a move, so a resting pointer cannot take the cursor", () => {
    expect(pointerMoved(null, { x: 10, y: 20 })).toBe(false);
  });

  it("ignores a repeat of the same coordinates (the list scrolled under the mouse)", () => {
    const at: Point = { x: 10, y: 20 };
    expect(pointerMoved(at, { x: 10, y: 20 })).toBe(false);
  });

  it("counts any change in either axis", () => {
    expect(pointerMoved({ x: 10, y: 20 }, { x: 11, y: 20 })).toBe(true);
    expect(pointerMoved({ x: 10, y: 20 }, { x: 10, y: 19 })).toBe(true);
  });

  it("walks a keyboard scroll with an unseeded, stationary pointer without a false move", () => {
    // Open with the mouse resting over the list; ↓ scrolls a row under it twice.
    let last: Point | null = null;
    const events = [
      { x: 300, y: 400 },
      { x: 300, y: 400 },
      { x: 300, y: 400 },
    ];
    const moves = events.map((e) => {
      const moved = pointerMoved(last, e);
      last = e;
      return moved;
    });
    expect(moves).toEqual([false, false, false]);
    // A real nudge afterwards is one.
    expect(pointerMoved(last, { x: 302, y: 400 })).toBe(true);
  });
});
