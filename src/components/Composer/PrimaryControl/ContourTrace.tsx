import { useEffect, useRef } from "react";

/** The pill's outer box and corner radius, in px. The trace runs 0.75 px inside
 *  the edge so a 1.25 px stroke sits on the contour. */
const SIZE = 28;
const RADIUS = 8;
const INSET = 0.75;
const SIDE = SIZE - 2 * INSET;
const CORNER = RADIUS - INSET;
/** Perimeter of the rounded square: four straights plus one full circle. */
const PERIMETER = 4 * SIDE - 8 * CORNER + 2 * Math.PI * CORNER;
/** One lap per 1.6 s, linear — a status indicator, not a spring. */
const LAP_MS = 1600;
/** The tail is a stack of short dashes, each a little more transparent than
 *  the one ahead of it, covering 36% of the perimeter. */
const SLICES = 18;
const TAIL = 0.36;
const SLICE = (TAIL * PERIMETER) / SLICES;

/** A short accent dash travelling around the running pill's contour — bright
 *  head, fading tail. Status and control in one: the agent is working, and this
 *  is where you stop it.
 *
 *  Driven by `requestAnimationFrame` writing `stroke-dashoffset` straight onto
 *  the rects: a CSS animation of a dash offset can't follow a rounded path
 *  smoothly, and this runs only while `active`. Under reduced motion the
 *  stylesheet turns it into a static outline and the loop never starts. */
export function ContourTrace({ active }: { active: boolean }) {
  const rects = useRef<(SVGRectElement | null)[]>([]);

  useEffect(() => {
    if (!active) return;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const t0 = performance.now();
    let frame = 0;
    const tick = (now: number) => {
      const head = (((now - t0) % LAP_MS) / LAP_MS) * PERIMETER;
      rects.current.forEach((el, i) => {
        el?.setAttribute("stroke-dashoffset", String(-(head - (i + 1) * SLICE)));
      });
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [active]);

  return (
    <svg className="pc-ring" viewBox={`0 0 ${SIZE} ${SIZE}`} aria-hidden="true">
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
          width={SIDE}
          height={SIDE}
          rx={CORNER}
          strokeDasharray={`${SLICE} ${PERIMETER - SLICE}`}
          style={{ opacity: (1 - i / SLICES) ** 1.6 }}
        />
      ))}
    </svg>
  );
}
