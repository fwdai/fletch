import { type ReactNode, useEffect, useRef, useState } from "react";

/** Animate a number from its previous value to `target` with an ease-out
 *  ramp. Renders the final value immediately under prefers-reduced-motion. */
export function useCountUp(target: number, durationMs = 700): number {
  const [value, setValue] = useState(0);
  // The number currently on screen — the animation's start point. Written only
  // alongside `setValue`, never by the effect up front: an effect that marked
  // the target as reached before the first frame ran was not re-runnable, and
  // React re-runs mount effects (StrictMode mounts, unmounts and mounts again,
  // cancelling the pending frame in between). The second run then saw "already
  // at target" and left the very first render's 0 on screen.
  const shownRef = useRef(0);

  useEffect(() => {
    const show = (v: number) => {
      shownRef.current = v;
      setValue(v);
    };
    if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) {
      show(target);
      return;
    }
    const from = shownRef.current;
    if (from === target) return;
    let raf = 0;
    const t0 = performance.now();
    const tick = (now: number) => {
      const p = Math.min(1, (now - t0) / durationMs);
      const eased = 1 - (1 - p) ** 3;
      show(Math.round(from + (target - from) * eased));
      if (p < 1) raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [target, durationMs]);

  return value;
}

/** A counting-up number. `format` defaults to locale grouping. */
export function CountUp({
  value,
  format = (n) => n.toLocaleString(),
}: {
  value: number;
  format?: (n: number) => string;
}) {
  return <>{format(useCountUp(value))}</>;
}

interface StatProps {
  /** Short unit label rendered after the value ("agents", "PRs", "tokens"). */
  label: string;
  /** Hover detail (e.g. "9 merged") via the app's data-tip tooltip. */
  tip?: string;
  /** Shimmer placeholder over the value while it computes. */
  loading?: boolean;
  /** The value — typically a CountUp (or several, for +/− pairs). */
  children?: ReactNode;
}

/** One inline value–label pair of a compact stat strip. Detail beyond the
 *  headline number lives in the hover tooltip, keeping the row one text
 *  line tall. */
export function Stat({ label, tip, loading, children }: StatProps) {
  return (
    <div className={`stat iflex-center${tip ? " tip" : ""}`} data-tip={tip}>
      <span className="stat-v mono">{loading ? <span className="stat-shimmer" /> : children}</span>
      <span className="stat-l">{label}</span>
    </div>
  );
}
