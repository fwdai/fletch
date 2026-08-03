import type { MouseEvent } from "react";

const MIN_WIDTH = 220;
/** Left pane stays bounded; the right pane can grow up to the center
 *  pane's width (computed per-drag below). */
const LEFT_MAX = 520;

/** Travel limits for one splitter. Omit either and the app shell's defaults
 *  apply — a 220px floor, and a ceiling of 520px on the left / half the shared
 *  space on the right.
 *
 *  `max` takes a function because a cap usually depends on how much room the
 *  *other* pane needs, which is only knowable from the live layout. It is
 *  called once at drag start with the splitter element, so a caller can measure
 *  its container or siblings. */
export interface SplitBounds {
  min?: number;
  max?: number | ((el: HTMLElement) => number);
}

/** Pane-resize drag handler. Returns an `onMouseDown` to attach to a
 *  splitter element; while dragging it sets the receiving width via
 *  `set`. `commit`, if given, fires once on drag end with the final width —
 *  use it to persist (the per-frame `set` stays in-memory only).
 *
 *  `current` may be a getter rather than a number, for a pane whose resting
 *  width is expressed in CSS (a percentage, say) and so isn't a number the
 *  caller holds. It is read once at drag start, off the live element. */
export function useSplitter(
  current: number | (() => number),
  set: (w: number) => void,
  side: "left" | "right",
  commit?: (w: number) => void,
  bounds?: SplitBounds,
) {
  return (e: MouseEvent<HTMLDivElement>) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = typeof current === "function" ? current() : current;
    let lastW = startW;
    const el = e.currentTarget;
    const min = bounds?.min ?? MIN_WIDTH;
    // The right pane may grow until it matches the center pane. With the
    // left pane and window fixed, `center + right` is constant for the
    // duration of the drag, so the cap is half their combined width —
    // measured once from the splitter's siblings (center precedes it,
    // the right pane follows it).
    let max: number;
    if (bounds?.max !== undefined) {
      max = typeof bounds.max === "function" ? bounds.max(el) : bounds.max;
    } else if (side === "right") {
      const center = el.previousElementSibling?.getBoundingClientRect().width ?? 0;
      const right = el.nextElementSibling?.getBoundingClientRect().width ?? startW;
      max = Math.floor((center + right) / 2);
    } else {
      max = LEFT_MAX;
    }
    // A caller's cap can come out below the floor on a small window; the floor
    // wins, exactly as the CSS `min-width`/`max-width` pair would resolve it.
    max = Math.max(min, max);
    el.classList.add("dragging");
    const move = (ev: globalThis.MouseEvent) => {
      const dx = ev.clientX - startX;
      const next = side === "left" ? startW + dx : startW - dx;
      lastW = Math.max(min, Math.min(max, next));
      set(lastW);
    };
    const up = () => {
      el.classList.remove("dragging");
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      if (lastW !== startW) commit?.(lastW);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };
}
