import { type ReactNode, useMemo, useState } from "react";
import { localDay } from "@/util/format";
import { buildHeatmapWeeks, heatLevel, monthLabels } from "./heatmap";

interface Props {
  /** Activity count per local day (YYYY-MM-DD). Missing days are 0. */
  counts: Record<string, number>;
  /** Grid width in weeks. 52 = one year, GitHub-style. */
  weeks?: number;
  /** Right edge of the grid; defaults to now. Injectable for tests. */
  endMs?: number;
  /** Tooltip / aria text for a day — the exact-value readout that keeps the
   *  chart legible without relying on color alone. */
  tooltipFor: (day: string, count: number) => string;
  /** Optional slot on the footer row, opposite the Less/More legend
   *  (e.g. a streak chip). */
  footer?: ReactNode;
}

interface Tip {
  x: number;
  y: number;
  text: string;
}

/** GitHub-style contribution grid on the app's accent ramp. One cell per
 *  local day, columns are Monday-first weeks, intensity scales to the
 *  busiest day in range. Hover any cell for the exact counts. */
export function ActivityHeatmap({ counts, weeks = 52, endMs, tooltipFor, footer }: Props) {
  const end = endMs ?? Date.now();
  const grid = useMemo(() => buildHeatmapWeeks(end, weeks, counts), [end, weeks, counts]);
  const months = useMemo(() => monthLabels(grid), [grid]);
  const max = useMemo(
    () => grid.reduce((m, col) => col.reduce((n, c) => Math.max(n, c.count), m), 0),
    [grid],
  );
  const today = localDay(end);
  const [tip, setTip] = useState<Tip | null>(null);

  return (
    <div className="heat">
      <div className="heat-months text-xs" aria-hidden="true">
        {months.map((label, i) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: fixed-length positional slots
          <span key={i} className="heat-mlabel">
            {label}
          </span>
        ))}
      </div>
      <div className="heat-body">
        <div className="heat-daylabels text-xs" aria-hidden="true">
          <span>Mon</span>
          <span>Wed</span>
          <span>Fri</span>
        </div>
        <div className="heat-grid" role="img" aria-label="Daily activity for the past year">
          {grid.map((col) => (
            <div key={col[0].day} className="heat-col">
              {col.map((cell) =>
                cell.inRange ? (
                  <div
                    key={cell.day}
                    className={`heat-cell${cell.day === today ? " today" : ""}`}
                    data-level={heatLevel(cell.count, max)}
                    aria-label={tooltipFor(cell.day, cell.count)}
                    onMouseEnter={(e) => {
                      const r = e.currentTarget.getBoundingClientRect();
                      setTip({
                        x: r.left + r.width / 2,
                        y: r.bottom + 6,
                        text: tooltipFor(cell.day, cell.count),
                      });
                    }}
                    onMouseLeave={() => setTip(null)}
                  />
                ) : (
                  <div key={cell.day} className="heat-cell out" />
                ),
              )}
            </div>
          ))}
        </div>
      </div>
      <div className="heat-foot">
        <div className="heat-foot-l text-xs">{footer}</div>
        <div className="heat-legend text-xs" aria-hidden="true">
          Less
          {[0, 1, 2, 3, 4].map((l) => (
            <span key={l} className="heat-cell" data-level={l} />
          ))}
          More
        </div>
      </div>
      {tip && (
        <div
          className="heat-tip mono text-xs"
          style={{ left: tip.x, top: tip.y }}
          role="presentation"
        >
          {tip.text}
        </div>
      )}
    </div>
  );
}
