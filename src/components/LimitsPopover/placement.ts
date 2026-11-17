/** The card's natural width, and the narrowest it gets before it stops
 *  shrinking with the sidebar. */
const CARD_WIDTH = 328;
const CARD_MIN_WIDTH = 280;
/** The gap between the card and the sidebar's edges (or the window's). */
const CARD_GUTTER = 8;

/** Where the usage card sits horizontally: centered over the sidebar, a gutter
 *  in from its edges, up to the card's natural width. A sidebar too narrow for
 *  the card's minimum leaves it at the window's edge, overhanging to the right;
 *  so does a hidden one (`sidebarWidth` 0), at full width. */
export function cardBox(sidebarWidth: number): { left: number; width: number } {
  if (sidebarWidth <= 0) return { left: CARD_GUTTER, width: CARD_WIDTH };
  const width = Math.min(CARD_WIDTH, Math.max(CARD_MIN_WIDTH, sidebarWidth - 2 * CARD_GUTTER));
  return { left: Math.max(CARD_GUTTER, Math.round((sidebarWidth - width) / 2)), width };
}
