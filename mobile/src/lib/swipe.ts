import { type RefObject, useEffect, useRef } from "react";

/** A drag that dismisses something: a screen swiped off to the right, a sheet
 *  pulled down. While the finger is down, `stage` carries `.dragging` and a
 *  `--swipe` custom property in 0..1, so the CSS that already animates the
 *  open/close can follow the finger instead. On release the property is
 *  cleared in the same frame the caller's own close kicks in, so the CSS
 *  transition picks up from wherever the finger let go. */
export interface SwipeOptions {
  /** `x` dismisses by dragging right, `y` by dragging down. */
  axis: "x" | "y";
  /** Only begin when the touch lands within this many px of the leading edge
   *  (left for `x`, top for `y`), and own such a touch outright like the iOS
   *  edge pan: nothing underneath scrolls while it lasts. Omit to begin
   *  anywhere on the target, in which case the first move decides between a
   *  swipe and a scroll. */
  edge?: number;
  enabled?: boolean;
  onCommit: () => void;
  /** Injected clock for tests. */
  now?: () => number;
}

/** A flick commits however short it was; a long drag commits unless it is
 *  being flung back. Velocity is px/ms along the axis. */
export const shouldCommit = (delta: number, size: number, velocity: number) =>
  velocity > 0.4 || (delta > size * 0.3 && velocity > -0.2);

/** How far an edge touch travels along the axis before it counts as a swipe.
 *  A finger's first reported move is a point or so of noise in any direction,
 *  so an edge gesture waits this long rather than reading the first sample. */
export const EDGE_SLOP = 10;

/** Anything between `target` and `root` that is scrolled along `axis` owns the
 *  gesture: a pulled-down sheet body must scroll back to top before it drags
 *  the sheet, the way iOS sheets behave. */
const scrolledAncestor = (target: EventTarget | null, root: HTMLElement, axis: "x" | "y") => {
  let node = target instanceof Element ? target : null;
  while (node && node !== root) {
    if ((axis === "x" ? node.scrollLeft : node.scrollTop) > 0) return true;
    node = node.parentElement;
  }
  return false;
};

export function attachSwipe(target: HTMLElement, stage: HTMLElement, opts: () => SwipeOptions) {
  let start: { x: number; y: number } | null = null;
  let active = false;
  let last = { d: 0, t: 0 };
  let velocity = 0;
  const now = () => opts().now?.() ?? performance.now();
  const size = () => (opts().axis === "x" ? target.offsetWidth : target.offsetHeight);

  const onStart = (e: TouchEvent) => {
    const o = opts();
    if (o.enabled === false || e.touches.length !== 1) return;
    const t = e.touches[0];
    const rect = target.getBoundingClientRect();
    const along = o.axis === "x" ? t.clientX - rect.left : t.clientY - rect.top;
    if (o.edge !== undefined && along > o.edge) return;
    if (scrolledAncestor(e.target, target, o.axis)) return;
    start = { x: t.clientX, y: t.clientY };
    active = false;
    last = { d: 0, t: now() };
    velocity = 0;
  };

  const onMove = (e: TouchEvent) => {
    if (!start) return;
    const o = opts();
    const t = e.touches[0];
    const dx = t.clientX - start.x;
    const dy = t.clientY - start.y;
    const along = o.axis === "x" ? dx : dy;
    const across = o.axis === "x" ? dy : dx;
    if (!active) {
      // WebKit lets only the first touchmove of a touch veto native scrolling,
      // so whatever is decided here is final for the whole touch.
      if (o.edge !== undefined) {
        // An edge touch is ours from its first move, scroll or not; it starts
        // dragging once it has clearly travelled the right way.
        e.preventDefault();
        if (along < EDGE_SLOP) return;
      } else {
        if (along === 0 && across === 0) return;
        // Anywhere else the first real movement decides: the wrong way or
        // mostly across the axis, and this is a scroll, not a swipe.
        if (along <= 0 || Math.abs(across) > Math.abs(along)) {
          start = null;
          return;
        }
      }
      active = true;
      stage.classList.add("dragging");
    }
    e.preventDefault();
    const d = Math.max(0, along);
    const t1 = now();
    const dt = t1 - last.t;
    if (dt > 0) velocity = 0.6 * ((d - last.d) / dt) + 0.4 * velocity;
    last = { d, t: t1 };
    stage.style.setProperty("--swipe", String(Math.min(1, d / size())));
  };

  const onEnd = () => {
    if (!start) return;
    const was = active;
    start = null;
    active = false;
    if (!was) return;
    stage.classList.remove("dragging");
    stage.style.removeProperty("--swipe");
    if (shouldCommit(last.d, size(), velocity)) opts().onCommit();
  };

  target.addEventListener("touchstart", onStart, { passive: true });
  target.addEventListener("touchmove", onMove, { passive: false });
  target.addEventListener("touchend", onEnd);
  target.addEventListener("touchcancel", onEnd);
  return () => {
    target.removeEventListener("touchstart", onStart);
    target.removeEventListener("touchmove", onMove);
    target.removeEventListener("touchend", onEnd);
    target.removeEventListener("touchcancel", onEnd);
    stage.classList.remove("dragging");
    stage.style.removeProperty("--swipe");
  };
}

/** `attachSwipe` for the life of `target`. `stage` receives the `--swipe`
 *  property and defaults to the target itself. */
export function useSwipe(
  target: RefObject<HTMLElement | null>,
  opts: SwipeOptions,
  stage?: RefObject<HTMLElement | null>,
) {
  const latest = useRef(opts);
  latest.current = opts;
  useEffect(() => {
    const el = target.current;
    if (!el) return;
    return attachSwipe(el, stage?.current ?? el, () => latest.current);
  }, [target, stage]);
}
