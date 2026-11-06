import { describe, expect, it } from "vitest";
import {
  acceptActions,
  CONCURRENCY_CHOICES,
  DEFAULT_MAX_CONCURRENT,
  flagOn,
  MAX_CONCURRENT_CEILING,
  parseCap,
} from "./autonomy";

describe("flagOn", () => {
  it("reads every spelling the host writes or recognizes", () => {
    for (const on of ["1", "true", "on", "yes", "TRUE", " On "]) {
      expect(flagOn(on, false)).toBe(true);
    }
    for (const off of ["0", "false", "off", "no", "FALSE", " Off "]) {
      expect(flagOn(off, true)).toBe(false);
    }
  });

  it("falls back on an absent, blank or unreadable value — in both directions", () => {
    // A dial nobody can parse is not a mandate: the default decides, so a
    // hand-edited row can't silently switch autonomy on.
    for (const raw of [undefined, "", "  ", "maybe"]) {
      expect(flagOn(raw, false)).toBe(false);
      expect(flagOn(raw, true)).toBe(true);
    }
  });
});

describe("parseCap", () => {
  it("shows the number actually in force, clamped like the host", () => {
    expect(parseCap(undefined)).toBe(DEFAULT_MAX_CONCURRENT);
    expect(parseCap("1")).toBe(1);
    expect(parseCap("4")).toBe(4);
    // Hand-edited above the ceiling: the select must show what the drainer will
    // really do, not the row's wishful number.
    expect(parseCap("12")).toBe(MAX_CONCURRENT_CEILING);
  });

  it("defaults on garbage, zero and fractions", () => {
    for (const bad of ["0", "-1", "two", "3.5", "", "1e3"]) {
      expect(parseCap(bad)).toBe(DEFAULT_MAX_CONCURRENT);
    }
  });

  it("offers every setting between the default and the ceiling", () => {
    expect(CONCURRENCY_CHOICES).toEqual([1, 2, 3, 4]);
  });
});

describe("acceptActions", () => {
  it("offers the one-click queue as a second action while the dial is off", () => {
    expect(acceptActions(false, "Accept")).toEqual({
      primary: "Accept",
      queue: "Accept & queue",
    });
    expect(acceptActions(false, "Accept all")).toEqual({
      primary: "Accept all",
      queue: "Accept all & queue",
    });
  });

  it("says what the primary does once the dial makes it queue", () => {
    // With autoqueue on, Accept *is* Accept & queue — a button that hid that
    // would be the one place the board understates what a click starts.
    expect(acceptActions(true, "Accept")).toEqual({
      primary: "Accept & queue",
      queue: null,
    });
    expect(acceptActions(true, "Accept all").queue).toBeNull();
  });
});
