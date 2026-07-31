import { describe, expect, it } from "vitest";
import { fitWithin } from "../../src/util/image";

describe("fitWithin", () => {
  it("leaves an image already inside the box untouched", () => {
    expect(fitWithin(800, 600, 1600)).toEqual({ w: 800, h: 600 });
    // Exactly at the cap counts as inside.
    expect(fitWithin(1600, 900, 1600)).toEqual({ w: 1600, h: 900 });
  });

  it("scales by the longest edge, preserving aspect ratio", () => {
    // A 2x retina capture of a 1512pt-wide display.
    expect(fitWithin(3024, 1890, 1600)).toEqual({ w: 1600, h: 1000 });
    // Portrait: the height is what gets clamped.
    expect(fitWithin(1890, 3024, 1600)).toEqual({ w: 1000, h: 1600 });
  });

  it("never returns a zero dimension", () => {
    // A canvas of width 0 throws, so an extreme aspect ratio must still floor
    // at one pixel rather than round down to nothing.
    expect(fitWithin(10000, 3, 100)).toEqual({ w: 100, h: 1 });
  });

  it("tolerates a degenerate zero-sized source", () => {
    expect(fitWithin(0, 0, 1600)).toEqual({ w: 0, h: 0 });
  });
});
