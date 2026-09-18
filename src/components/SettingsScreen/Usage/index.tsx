import { useMemo, useState } from "react";
import { rangeBounds, type UsageMetric, type UsageRange, useUsageStats } from "@/data/usage";
import { UsageBreakdown } from "./UsageBreakdown";
import { UsageChart } from "./UsageChart";
import { UsageHeader } from "./UsageHeader";
import { UsageSummary } from "./UsageSummary";
import { UsageTotals } from "./UsageTotals";

/** What every Claude Code and Codex session on this machine has burned, read
 *  from the transcripts on disk rather than from anything Fletch itself ran —
 *  so sessions started outside the app count too. */
export function UsagePane() {
  const [range, setRange] = useState<UsageRange>("30d");
  const [metric, setMetric] = useState<UsageMetric>("cost");
  const { stats, loading, refreshing, error, refresh, scannedAt, catalogReady } =
    useUsageStats(range);

  // The header describes the *selected* window immediately; `stats.range` is
  // the window the data in hand covers, which lags by one scan.
  const bounds = useMemo(() => rangeBounds(range, Date.now()), [range]);
  const empty = !!stats?.empty;
  // A failed first scan leaves nothing to show but the error; a failed re-scan
  // still has the previous numbers behind it.
  const showContent = stats !== null || loading;

  return (
    <div className="set-pane">
      <UsageHeader
        range={range}
        onRange={setRange}
        metric={metric}
        onMetric={setMetric}
        bounds={bounds}
        busy={loading || refreshing}
        scannedAt={scannedAt}
        onRefresh={refresh}
      />

      {/* No catalog, no rates: every bucket prices to null, so the pane would
          otherwise be a wall of "Unpriced". Say why once, at the top. */}
      {!catalogReady && !empty && (
        <div className="usg-hint text-sm">
          Prices not loaded yet — cost appears once the model catalog loads.
        </div>
      )}

      {error && (
        <div className="usg-state error text-sm">
          Couldn’t read the transcripts: {error}.{" "}
          {stats ? "Showing the last scan." : "Try refreshing."}
        </div>
      )}

      {/* Dimmed rather than replaced while a re-scan runs: the figures stay
          readable and comparable, but visibly provisional. */}
      <div className="usg-content" aria-busy={refreshing || undefined}>
        {showContent && empty && (
          <div className="usg-state text-sm">
            No Claude Code or Codex usage in this window.{" "}
            {stats.scannedFiles > 0
              ? `Read ${stats.scannedFiles.toLocaleString()} transcript files — none of them recorded tokens here.`
              : "No transcripts were found on this machine yet."}
          </div>
        )}

        {showContent && !empty && (
          <>
            <section className="set-group usg-top">
              <UsageSummary stats={stats} metric={metric} loading={loading} />
              <UsageChart stats={stats} metric={metric} loading={loading} />
            </section>

            <UsageTotals stats={stats} loading={loading} />
            <UsageBreakdown stats={stats} loading={loading} />

            <p className="usg-foot text-xs">
              Costs are list API prices from models.dev; subscription plans are not billed per
              token.
            </p>
          </>
        )}
      </div>
    </div>
  );
}
