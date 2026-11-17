import { describe, expect, it } from "vitest";
import { cardBox } from "./placement";

describe("cardBox", () => {
  it("centers the card at its natural width over a wide sidebar", () => {
    expect(cardBox(520)).toEqual({ left: 96, width: 328 });
  });

  it("insets the card 8px from each edge of a sidebar narrower than it", () => {
    // The default sidebar width.
    expect(cardBox(312)).toEqual({ left: 8, width: 296 });
  });

  it("stops shrinking at its minimum and keeps to the window's edge", () => {
    expect(cardBox(220)).toEqual({ left: 8, width: 280 });
  });

  it("sits at the window's edge at full width with the sidebar hidden", () => {
    expect(cardBox(0)).toEqual({ left: 8, width: 328 });
  });
});
