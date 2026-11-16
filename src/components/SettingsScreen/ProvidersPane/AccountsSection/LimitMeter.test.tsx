// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { render, screen } from "@/test/render";
import { LimitMeter } from "./LimitMeter";

const nowMs = 1_000_000;
const inTwoHours = nowMs / 1000 + 2 * 3_600;

describe("LimitMeter compact", () => {
  it("hangs the reset line off the time left, not the whole row", () => {
    render(
      <LimitMeter
        label="5h"
        window={{ percent: 40, resets_at: inTwoHours }}
        nowMs={nowMs}
        compact
      />,
    );
    const left = screen.getByText("2h");
    expect(left.getAttribute("data-tip")).toMatch(/^resets .+ · in 2h$/);
    expect(left.closest(".set-prov-limit")?.hasAttribute("data-tip")).toBe(false);
    expect(screen.getByText("40%").hasAttribute("data-tip")).toBe(false);
  });

  it("puts a note on the share when the window has no reset time", () => {
    render(
      <LimitMeter
        label="7d Fable"
        window={{ percent: 0, resets_at: null }}
        nowMs={nowMs}
        note="as of 2h ago"
        compact
      />,
    );
    expect(screen.getByText("0%").getAttribute("data-tip")).toBe("as of 2h ago");
  });
});
