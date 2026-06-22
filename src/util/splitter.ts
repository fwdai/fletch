import type { MouseEvent } from "react";

const MIN_WIDTH = 220;
/** Left pane stays bounded; the right pane can grow up to the center
 *  pane's width (computed per-drag below). */
const LEFT_MAX = 520;

/** Pane-resize drag handler. Returns an `onMouseDown` to attach to a
 *  splitter element; while dragging it sets the receiving width via
 *  `set`. Left width is clamped to [220, 520]; the right pane may expand
 *  until it is as wide as the center pane. `commit`, if given, fires once
 *  on drag end with the final width — use it to persist (the per-frame
 *  `set` stays in-memory only). */
export function useSplitter(
  current: number,
  set: (w: number) => void,
  side: "left" | "right",
  commit?: (w: number) => void,
) {
  return (e: MouseEvent<HTMLDivElement>) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = current;
    let lastW = current;
    const el = e.currentTarget;
    // The right pane may grow until it matches the center pane. With the
    // left pane and window fixed, `center + right` is constant for the
    // duration of the drag, so the cap is half their combined width —
    // measured once from the splitter's siblings (center precedes it,
    // the right pane follows it).
    let max = LEFT_MAX;
    if (side === "right") {
      const center = el.previousElementSibling?.getBoundingClientRect().width ?? 0;
      const right = el.nextElementSibling?.getBoundingClientRect().width ?? startW;
      max = Math.max(MIN_WIDTH, Math.floor((center + right) / 2));
    }
    el.classList.add("dragging");
    const move = (ev: globalThis.MouseEvent) => {
      const dx = ev.clientX - startX;
      const next = side === "left" ? startW + dx : startW - dx;
      lastW = Math.max(MIN_WIDTH, Math.min(max, next));
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
