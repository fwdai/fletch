/** Placement math for the app-wide tooltip bubble. Pure so it can be tested
 *  without a DOM; `TooltipLayer` feeds it `getBoundingClientRect` numbers. */

export interface Box {
  top: number;
  left: number;
  width: number;
  height: number;
}

export interface Size {
  width: number;
  height: number;
}

export interface Placement {
  top: number;
  left: number;
  /** Which side of the trigger the bubble landed on. */
  side: "above" | "below";
}

export interface PlaceOptions {
  /** The side tried first; the other is the fallback when it does not fit. */
  prefer?: "above" | "below";
  /** Centre the bubble on the trigger, or line up their left edges. */
  align?: "center" | "start";
}

/** Space between the trigger's edge and the bubble. */
export const TIP_GAP = 6;
/** Minimum distance the bubble keeps from the viewport edges. */
export const TIP_MARGIN = 8;

/** Centre the bubble above the trigger; drop it below when the top of the
 *  viewport is too close; then slide it horizontally so it stays on screen.
 *  `prefer: "below"` tries below first and goes above only when that is the
 *  side with room; `align: "start"` lines the left edges up instead. */
export function placeTooltip(
  trigger: Box,
  bubble: Size,
  viewport: Size,
  { prefer = "above", align = "center" }: PlaceOptions = {},
): Placement {
  const fitsAbove = trigger.top - TIP_GAP - bubble.height >= TIP_MARGIN;
  const fitsBelow =
    trigger.top + trigger.height + TIP_GAP + bubble.height <= viewport.height - TIP_MARGIN;
  const above = prefer === "above" ? fitsAbove : fitsAbove && !fitsBelow;
  const side = above ? "above" : "below";
  const top =
    side === "above"
      ? trigger.top - TIP_GAP - bubble.height
      : trigger.top + trigger.height + TIP_GAP;

  const wanted =
    align === "start" ? trigger.left : trigger.left + trigger.width / 2 - bubble.width / 2;
  const maxLeft = viewport.width - TIP_MARGIN - bubble.width;
  const left = Math.max(TIP_MARGIN, Math.min(wanted, maxLeft));

  return { top, left, side };
}
