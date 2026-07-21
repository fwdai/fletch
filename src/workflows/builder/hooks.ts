import { useEffect, useRef } from "react";

/** Dismiss a rect-anchored popover/menu when the viewport shifts under it.
 *
 *  The builder's popovers are fixed-positioned from a rect captured at open
 *  time, which goes stale once the canvas scrolls or the window resizes (the
 *  trigger moves, the fixed element doesn't). While `open`, listen on both and
 *  invoke `close`. Capture-phase catches scrolls inside the horizontal canvas
 *  scroller, which don't bubble. `close` is read through a ref so the listeners
 *  are attached only when `open` flips — not on every render.
 *
 *  Scrolls that originate *inside* the popover (a `.dd` menu with its own
 *  overflow) must NOT dismiss it — otherwise scrolling a long list snaps the
 *  menu shut and leaks the wheel to the page. Those are filtered by target. */
export function useDismissOnViewportChange(open: boolean, close: () => void): void {
  const closeRef = useRef(close);
  closeRef.current = close;
  useEffect(() => {
    if (!open) return;
    const onScroll = (e: Event) => {
      const t = e.target;
      if (t instanceof Element && t.closest(".dd")) return;
      closeRef.current();
    };
    const onResize = () => closeRef.current();
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onResize);
    };
  }, [open]);
}
