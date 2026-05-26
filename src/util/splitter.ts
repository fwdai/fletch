import type { MouseEvent } from "react";

/** Pane-resize drag handler. Returns an `onMouseDown` to attach to a
 *  splitter element; while dragging it sets the receiving width via
 *  `set`. Width is clamped to [220, 520]. */
export function useSplitter(
  current: number,
  set: (w: number) => void,
  side: "left" | "right",
) {
  return (e: MouseEvent<HTMLDivElement>) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = current;
    const el = e.currentTarget;
    el.classList.add("dragging");
    const move = (ev: globalThis.MouseEvent) => {
      const dx = ev.clientX - startX;
      const next = side === "left" ? startW + dx : startW - dx;
      set(Math.max(220, Math.min(520, next)));
    };
    const up = () => {
      el.classList.remove("dragging");
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };
}
