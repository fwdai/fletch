import { type RefObject, useLayoutEffect, useState } from "react";

/** Bottom edge the menu must fit within: the nearest scrolling ancestor (which
 *  clips absolutely-positioned descendants — e.g. the chat's `overflow-y: auto`
 *  scroller, below which the composer sits), clamped to the viewport. */
function clipBottom(el: HTMLElement): number {
  for (let node = el.parentElement; node; node = node.parentElement) {
    const overflowY = getComputedStyle(node).overflowY;
    if (overflowY === "auto" || overflowY === "scroll") {
      return Math.min(node.getBoundingClientRect().bottom, window.innerHeight);
    }
  }
  return window.innerHeight;
}

/** Which way an open menu goes: down unless its trigger (`wrap`) sits too
 *  close to the clip edge for the list (`menu`) to fit below — then up.
 *  Measured after render, before paint. */
export function usePlacement(
  open: boolean,
  wrapRef: RefObject<HTMLElement | null>,
  menuRef: RefObject<HTMLElement | null>,
): "down" | "up" {
  const [placement, setPlacement] = useState<"down" | "up">("down");
  useLayoutEffect(() => {
    if (!open) return;
    const wrap = wrapRef.current;
    const menu = menuRef.current;
    if (!wrap || !menu) return;
    const trigger = wrap.getBoundingClientRect();
    const menuHeight = menu.offsetHeight;
    const spaceBelow = clipBottom(wrap) - trigger.bottom;
    const spaceAbove = trigger.top;
    // Flip up only when it doesn't fit below *and* there's more room above, so a
    // cramped-both-ways menu still opens down (its natural, expected direction).
    setPlacement(spaceBelow < menuHeight + 8 && spaceAbove > spaceBelow ? "up" : "down");
  }, [open, wrapRef, menuRef]);
  return placement;
}
