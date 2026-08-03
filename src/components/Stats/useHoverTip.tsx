import { type MouseEvent, type ReactNode, useCallback, useState } from "react";

// A hover readout for chart marks. Every chart here encodes a value as an area
// or a color, both of which are approximate by nature and unreadable to anyone
// who can't distinguish the ramp — so each mark carries the exact numbers in
// text, on hover and in its aria-label.
//
// Fixed-position rather than an absolutely-positioned child, so the page's
// scroll container (`.ps-content`) can't clip it at the top of a tall bar.

interface Tip {
  x: number;
  y: number;
  text: string;
}

export interface HoverTip {
  /** Anchor the tip under the hovered element. */
  show: (e: MouseEvent<HTMLElement>, text: string) => void;
  hide: () => void;
  /** Render this at the end of the chart; null when nothing is hovered. */
  node: ReactNode;
}

export function useHoverTip(): HoverTip {
  const [tip, setTip] = useState<Tip | null>(null);

  const show = useCallback((e: MouseEvent<HTMLElement>, text: string) => {
    const r = e.currentTarget.getBoundingClientRect();
    setTip({ x: r.left + r.width / 2, y: r.bottom + 6, text });
  }, []);

  const hide = useCallback(() => setTip(null), []);

  return {
    show,
    hide,
    node: tip ? (
      <div
        className="hover-tip mono text-xs"
        style={{ left: tip.x, top: tip.y }}
        role="presentation"
      >
        {tip.text}
      </div>
    ) : null,
  };
}
