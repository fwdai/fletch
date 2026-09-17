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

/** Space between the trigger's edge and the bubble. */
export const TIP_GAP = 6;
/** Minimum distance the bubble keeps from the viewport edges. */
export const TIP_MARGIN = 8;

/** Centre the bubble above the trigger; drop it below when the top of the
 *  viewport is too close; then slide it horizontally so it stays on screen. */
export function placeTooltip(trigger: Box, bubble: Size, viewport: Size): Placement {
  const fitsAbove = trigger.top - TIP_GAP - bubble.height >= TIP_MARGIN;
  const side = fitsAbove ? "above" : "below";
  const top =
    side === "above"
      ? trigger.top - TIP_GAP - bubble.height
      : trigger.top + trigger.height + TIP_GAP;

  const centred = trigger.left + trigger.width / 2 - bubble.width / 2;
  const maxLeft = viewport.width - TIP_MARGIN - bubble.width;
  const left = Math.max(TIP_MARGIN, Math.min(centred, maxLeft));

  return { top, left, side };
}
