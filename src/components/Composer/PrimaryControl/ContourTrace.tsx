import { useEffect, useRef } from "react";

/** The trace runs 0.75 px inside the edge so a 1.25 px stroke sits on the
 *  contour. */
const INSET = 0.75;
/** The tail is a stack of short dashes, each a little more transparent than
 *  the one ahead of it, covering 36% of the perimeter. */
const SLICES = 18;
const TAIL = 0.36;

interface Props {
  active: boolean;
  /** The box the trace runs around, in px. The desktop pill is 28 with r8; the
   *  phone's disc is 40 with r20 — a circle. */
  size?: number;
  radius?: number;
  /** One lap, linear — a status indicator, not a spring. */
  lapMs?: number;
}

/** A short accent dash travelling around the running control's contour —
 *  bright head, fading tail. Status and control in one: the agent is working,
 *  and this is where you stop it.
 *
 *  Driven by `requestAnimationFrame` writing `stroke-dashoffset` straight onto
 *  the rects: a CSS animation of a dash offset can't follow a rounded path
 *  smoothly, and this runs only while `active`. Under reduced motion the
 *  stylesheet turns it into a static outline and the loop never starts. */
export function ContourTrace({ active, size = 28, radius = 8, lapMs = 1600 }: Props) {
  const rects = useRef<(SVGRectElement | null)[]>([]);
  const side = size - 2 * INSET;
  const corner = radius - INSET;
  // Four straights plus one full circle.
  const perimeter = 4 * side - 8 * corner + 2 * Math.PI * corner;
  const slice = (TAIL * perimeter) / SLICES;

  useEffect(() => {
    if (!active) return;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const t0 = performance.now();
    let frame = 0;
    const tick = (now: number) => {
      const head = (((now - t0) % lapMs) / lapMs) * perimeter;
      rects.current.forEach((el, i) => {
        el?.setAttribute("stroke-dashoffset", String(-(head - (i + 1) * slice)));
      });
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [active, lapMs, perimeter, slice]);

  return (
    <svg
      className="pc-ring"
      viewBox={`0 0 ${size} ${size}`}
      style={{ width: size, height: size }}
      aria-hidden="true"
    >
      {Array.from({ length: SLICES }, (_, i) => (
        <rect
          // Fixed slots along the tail: the index is the identity.
          // biome-ignore lint/suspicious/noArrayIndexKey: see above
          key={i}
          ref={(el) => {
            rects.current[i] = el;
          }}
          x={INSET}
          y={INSET}
          width={side}
          height={side}
          rx={corner}
          strokeDasharray={`${slice} ${perimeter - slice}`}
          style={{ opacity: (1 - i / SLICES) ** 1.6 }}
        />
      ))}
    </svg>
  );
}
