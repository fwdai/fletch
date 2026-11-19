// The rule that decides whether a log goes on following its bottom after a
// scroll event. The observers and listeners around it need a real layout, so
// they are exercised in a browser; this is the part that is pure.

import { describe, expect, it } from "vitest";
import { followsAfterScroll } from "@/util/stickToBottom";

describe("followsAfterScroll", () => {
  it("lets go on any scroll up, however small", () => {
    expect(followsAfterScroll(true, 992, 1000, 8)).toBe(false);
  });

  it("takes up following when a scroll down comes near the bottom", () => {
    expect(followsAfterScroll(false, 970, 900, 30)).toBe(true);
    expect(followsAfterScroll(false, 700, 600, 300)).toBe(false);
  });

  it("keeps following when the content shrinks and the view is clamped to the bottom", () => {
    // The offset went down, as in a scroll up, but it landed on the bottom.
    expect(followsAfterScroll(true, 700, 1000, 0)).toBe(true);
  });

  it("follows again once the user is back on the bottom", () => {
    expect(followsAfterScroll(false, 1000, 990, 0)).toBe(true);
  });

  it("leaves the state alone for a scroll down that stays far from the bottom", () => {
    expect(followsAfterScroll(true, 500, 400, 600)).toBe(true);
    expect(followsAfterScroll(false, 500, 400, 600)).toBe(false);
  });
});
