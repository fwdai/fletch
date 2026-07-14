import { useEffect, useRef } from "react";

/** Dismiss a rect-anchored popover/menu when the viewport shifts under it.
 *
 *  The builder's popovers are fixed-positioned from a rect captured at open
 *  time, which goes stale once the canvas scrolls or the window resizes (the
 *  trigger moves, the fixed element doesn't). While `open`, listen on both and
 *  invoke `close`. Capture-phase catches scrolls inside the horizontal canvas
 *  scroller, which don't bubble. `close` is read through a ref so the listeners
 *  are attached only when `open` flips — not on every render. */
export function useDismissOnViewportChange(open: boolean, close: () => void): void {
  const closeRef = useRef(close);
  closeRef.current = close;
  useEffect(() => {
    if (!open) return;
    const handler = () => closeRef.current();
    window.addEventListener("scroll", handler, true);
    window.addEventListener("resize", handler);
    return () => {
      window.removeEventListener("scroll", handler, true);
      window.removeEventListener("resize", handler);
    };
  }, [open]);
}
