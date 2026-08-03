import type { ReactNode } from "react";
import { Skeleton } from "./Skeleton";
import { useHoverTip } from "./useHoverTip";

/** What the loaded chart occupies: the `--mb-h` grid (72px) plus the axis row
 *  under it. Kept in sync by hand so the skeleton reserves the same space and
 *  the section doesn't jump when the data lands. */
const CHART_H = 92;

/** One column of the chart. */
export interface MiniBar {
  key: string;
  /** The plotted value. `null` means *no observation* — drawn as a baseline
   *  tick rather than a zero-height bar, because "we never looked" and "there
   *  was none" are different facts and a flat bar would state the second. */
  value: number | null;
  /** An optional larger value drawn behind `value` on the same scale, for the
   *  part-of-whole case (PRs merged inside PRs opened). */
  backdrop?: number | null;
  /** Exact numbers for hover + aria — the readout that keeps the chart legible
   *  without relying on bar height or color. */
  tip: string;
  /** Sparse x-axis label; omit or pass "" on the unlabeled majority. */
  label?: string;
}

interface Props {
  bars: MiniBar[];
  /** Describes the series for screen readers, e.g. "Tokens used per day". */
  ariaLabel: string;
  /** The data hasn't arrived yet. Handled here rather than left to callers
   *  because the obvious caller-side spelling — pass `[]` until it loads —
   *  is indistinguishable from a genuinely empty series, so every chart would
   *  flash its "nothing here" copy on the way to rendering fine. */
  loading?: boolean;
  /** Rendered in place of the grid when there is nothing to plot. */
  empty?: ReactNode;
  /** Legend / caption slot under the axis. */
  footer?: ReactNode;
}

/** A compact column chart on the accent ramp: one bar per bucket, scaled to
 *  the tallest value in range, exact figures on hover.
 *
 *  Deliberately axis-free apart from sparse tick labels — these sit inside a
 *  page section under a headline stat row, where the shape over time is the
 *  point and the precise numbers are one hover away. */
export function MiniBars({ bars, ariaLabel, loading, empty, footer }: Props) {
  const tip = useHoverTip();
  const max = bars.reduce((m, b) => Math.max(m, b.value ?? 0, b.backdrop ?? 0), 0);

  if (loading) return <Skeleton height={CHART_H} className="mb-skel" />;
  // Nothing anywhere in range — every bucket is zero or unobserved. A grid of
  // empty columns states that badly; say it in words instead.
  if (empty !== undefined && max <= 0) {
    return <div className="mb-empty text-sm">{empty}</div>;
  }

  // A bar rounds up to a visible sliver so a real-but-tiny value never renders
  // as the same nothing a zero does.
  const height = (v: number) => (v <= 0 ? 0 : Math.max(2, Math.round((v / max) * 100)));

  return (
    <div className="mb">
      <div className="mb-grid" role="img" aria-label={ariaLabel}>
        {bars.map((b) => (
          <div
            key={b.key}
            className="mb-col"
            aria-label={b.tip}
            onMouseEnter={(e) => tip.show(e, b.tip)}
            onMouseLeave={tip.hide}
          >
            <div className="mb-track">
              {b.value == null ? (
                <span className="mb-gap" />
              ) : (
                <>
                  {b.backdrop != null && b.backdrop > 0 && (
                    <span className="mb-bar back" style={{ height: `${height(b.backdrop)}%` }} />
                  )}
                  <span className="mb-bar" style={{ height: `${height(b.value)}%` }} />
                </>
              )}
            </div>
          </div>
        ))}
      </div>
      <div className="mb-axis text-xs" aria-hidden="true">
        {bars.map((b) => (
          <span key={b.key} className="mb-tick">
            {b.label ?? ""}
          </span>
        ))}
      </div>
      {footer && <div className="mb-foot text-xs">{footer}</div>}
      {tip.node}
    </div>
  );
}
