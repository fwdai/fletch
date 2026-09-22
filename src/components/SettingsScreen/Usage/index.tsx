import { useEffect, useMemo, useState } from "react";
import {
  ALL_HOSTS,
  rangeBounds,
  type UsageMetric,
  type UsageRange,
  useUsageStats,
} from "@/data/usage";
import { SetSeg } from "../primitives";
import { UsageBreakdown } from "./UsageBreakdown";
import { UsageChart } from "./UsageChart";
import { UsageGroupHead } from "./UsageGroupHead";
import { UsageHeader } from "./UsageHeader";
import { UsageSummary } from "./UsageSummary";
import { UsageTotals } from "./UsageTotals";

const METRICS: { value: UsageMetric; label: string }[] = [
  { value: "cost", label: "Cost" },
  { value: "tokens", label: "Tokens" },
];

/** What every Claude Code and Codex session has burned — on this machine and on
 *  every connected host — read from the transcripts on those disks rather than
 *  from anything Fletch itself ran, so sessions started outside the app count
 *  too. The host filter narrows it to one machine; with only this one, there is
 *  nothing to filter and the control stays hidden. */
export function UsagePane() {
  const [range, setRange] = useState<UsageRange>("30d");
  const [metric, setMetric] = useState<UsageMetric>("cost");
  const [host, setHost] = useState<string>(ALL_HOSTS);
  const { stats, hosts, loading, refreshing, error, refresh, scannedAt, catalogReady } =
    useUsageStats(range, host);

  // A host picked and then disconnected would leave the pane filtered to
  // nothing, with no control to undo it once the filter hides itself again.
  useEffect(() => {
    if (host !== ALL_HOSTS && !hosts.some((h) => h.id === host)) setHost(ALL_HOSTS);
  }, [host, hosts]);

  // The header names the window the numbers on screen actually cover, so it
  // reads `stats.range` — already clamped to the scans behind it — rather than
  // recomputing the selected window and claiming days no host answered for.
  // Before the first scan lands there are no numbers to describe, only the
  // window the control selects.
  const bounds = useMemo(() => stats?.range ?? rangeBounds(range, Date.now()), [stats, range]);
  const empty = !!stats?.empty;
  // A failed first scan leaves nothing to show but the error; a failed re-scan
  // still has the previous numbers behind it.
  const showContent = stats !== null || loading;

  return (
    <div className="set-pane">
      <UsageHeader
        range={range}
        onRange={setRange}
        hosts={hosts}
        host={host}
        onHost={setHost}
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
            {/* Cost / Tokens only re-reads this section — the totals and the
                breakdown always show both — so the switch sits on its heading,
                nested under the page-wide period picker in the header. */}
            <section className="set-group">
              <UsageGroupHead label="Overview">
                <SetSeg<UsageMetric> value={metric} options={METRICS} onChange={setMetric} />
              </UsageGroupHead>
              <div className="usg-top">
                <UsageSummary stats={stats} metric={metric} loading={loading} />
                <UsageChart stats={stats} metric={metric} loading={loading} />
              </div>
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
