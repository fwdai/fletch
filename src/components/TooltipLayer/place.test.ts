import { describe, expect, it } from "vitest";
import { placeTooltip, TIP_GAP, TIP_MARGIN } from "./place";

const viewport = { width: 1000, height: 800 };
const bubble = { width: 120, height: 24 };

describe("placeTooltip", () => {
  it("centres the bubble above the trigger when there is room", () => {
    const p = placeTooltip({ top: 300, left: 400, width: 40, height: 20 }, bubble, viewport);
    expect(p.side).toBe("above");
    expect(p.top).toBe(300 - TIP_GAP - bubble.height);
    // trigger centre 420, bubble half-width 60
    expect(p.left).toBe(360);
  });

  it("flips below when the trigger hugs the top of the viewport", () => {
    const p = placeTooltip({ top: 10, left: 400, width: 40, height: 20 }, bubble, viewport);
    expect(p.side).toBe("below");
    expect(p.top).toBe(10 + 20 + TIP_GAP);
  });

  it("keeps the bubble inside the right edge of the viewport", () => {
    // A sidebar-style trigger flush against the right edge.
    const p = placeTooltip({ top: 300, left: 980, width: 20, height: 20 }, bubble, viewport);
    expect(p.left).toBe(viewport.width - TIP_MARGIN - bubble.width);
    expect(p.left + bubble.width).toBeLessThanOrEqual(viewport.width - TIP_MARGIN);
  });

  it("keeps the bubble inside the left edge of the viewport", () => {
    const p = placeTooltip({ top: 300, left: 0, width: 20, height: 20 }, bubble, viewport);
    expect(p.left).toBe(TIP_MARGIN);
  });

  it("places exactly at the threshold where the bubble still fits above", () => {
    const top = TIP_MARGIN + TIP_GAP + bubble.height;
    expect(placeTooltip({ top, left: 400, width: 40, height: 20 }, bubble, viewport).side).toBe(
      "above",
    );
    expect(
      placeTooltip({ top: top - 1, left: 400, width: 40, height: 20 }, bubble, viewport).side,
    ).toBe("below");
  });

  it("opens below with left edges aligned when asked to", () => {
    const p = placeTooltip({ top: 300, left: 400, width: 40, height: 20 }, bubble, viewport, {
      prefer: "below",
      align: "start",
    });
    expect(p.side).toBe("below");
    expect(p.top).toBe(300 + 20 + TIP_GAP);
    expect(p.left).toBe(400);
  });

  it("goes above a trigger near the bottom edge even when below is preferred", () => {
    const p = placeTooltip({ top: 770, left: 400, width: 40, height: 20 }, bubble, viewport, {
      prefer: "below",
    });
    expect(p.side).toBe("above");
    expect(p.top).toBe(770 - TIP_GAP - bubble.height);
  });

  it("stays below when preferred and neither side has room", () => {
    const short = { width: 1000, height: 60 };
    const p = placeTooltip({ top: 20, left: 400, width: 40, height: 20 }, bubble, short, {
      prefer: "below",
    });
    expect(p.side).toBe("below");
  });

  it("slides a start-aligned bubble back inside the right edge", () => {
    const p = placeTooltip({ top: 300, left: 950, width: 20, height: 20 }, bubble, viewport, {
      align: "start",
    });
    expect(p.left).toBe(viewport.width - TIP_MARGIN - bubble.width);
  });
});
