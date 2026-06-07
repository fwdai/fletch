// Onboarding ambient background — breathing topographic contour layers with
// parallax, an accent glow, vignette, and faint grain. Ported from the design
// prototype (onboarding/app.jsx). Parallax follows the current step `phase`.

import { useMemo } from "react";
import type { CSSProperties } from "react";

interface Ridge {
  d: string;
  opacity: number;
  dur: number;
}

export function Ambient({ phase }: { phase: number }) {
  const ridges = useMemo<Ridge[]>(() => {
    let seed = 21;
    const rng = () => {
      seed = (seed * 9301 + 49297) % 233280;
      return seed / 233280;
    };
    const make = (yMid: number, amp: number, steps: number) => {
      const pts: string[] = [];
      for (let i = 0; i <= steps; i++) {
        const x = (i / steps) * 100;
        const y = yMid + Math.sin(i * 0.5 + rng() * 1.8) * amp + (rng() - 0.5) * 1.1;
        pts.push(`${x.toFixed(2)},${y.toFixed(2)}`);
      }
      return `M ${pts.join(" L ")}`;
    };
    const rows: Ridge[] = [];
    const count = 9;
    for (let i = 0; i < count; i++) {
      const t = i / (count - 1);
      const y = 8 + t * 86;
      const amp = 2.4 + (1 - Math.abs(t - 0.5) * 2) * 4.5;
      rows.push({
        d: make(y, amp, 26),
        opacity: 0.05 + (1 - Math.abs(t - 0.42) * 1.6) * 0.12,
        dur: 34 + (i % 4) * 9,
      });
    }
    return rows;
  }, []);

  const par = phase;
  return (
    <div className="ob-ambient">
      <div
        className={`ob-glow ${phase > 0 ? "lo" : ""}`}
        style={{ transform: `translateX(-50%) translateY(${par * -1.6}%)` }}
      />
      {/* The per-step parallax transform lives on this wrapping <div>, not the
          <svg>: WebKit (Tauri's macOS WKWebView) jumps instead of animating CSS
          transform transitions on SVG root elements, so the lines would shift
          sharply. A <div> transitions reliably. */}
      <div
        className="ob-contours"
        style={{ transform: `translateY(${par * -1.3}%) translateX(${par * -0.6}%)` }}
      >
      <svg
        className="ob-contours-svg"
        viewBox="0 0 100 100"
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        <defs>
          <linearGradient id="ob-fade" x1="0" y1="0" x2="1" y2="0">
            <stop offset="0%" stopColor="#fff" stopOpacity="0" />
            <stop offset="16%" stopColor="#fff" stopOpacity="1" />
            <stop offset="84%" stopColor="#fff" stopOpacity="1" />
            <stop offset="100%" stopColor="#fff" stopOpacity="0" />
          </linearGradient>
          <mask id="ob-mask">
            <rect width="100" height="100" fill="url(#ob-fade)" />
          </mask>
        </defs>
        <g mask="url(#ob-mask)">
          {ridges.map((r, i) => (
            <path
              key={i}
              className="cl drift"
              d={r.d}
              style={{ opacity: r.opacity, animationDuration: `${r.dur}s` } as CSSProperties}
            />
          ))}
        </g>
      </svg>
      </div>
      <div className="ob-vignette" />
      <div className="ob-grain" />
    </div>
  );
}
