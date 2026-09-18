import { formatDayTick, formatHeatDay, type MiniBar, MiniBars } from "@/components/Stats";
import { providerLabel } from "@/data/providers";
import {
  coverageLabel,
  type UsageDay,
  type UsageDaySlice,
  type UsageMetric,
  type UsageStats,
} from "@/data/usage";
import { formatTokens } from "@/util/format";

/** Roughly six ticks whatever the range, so 24h/7d label every column and 90d
 *  labels every other week. */
const tickEvery = (days: number) => Math.max(1, Math.ceil(days / 6));

const value = (d: UsageDay, metric: UsageMetric) => (metric === "cost" ? d.costUsd : d.tokens);

/** A day or one provider's part of it, in the selected metric. Cost goes
 *  through `coverageLabel`, so an unpriced slice reads "Unpriced" and a
 *  partly-priced one "≥ $x" rather than a total it can't back up. */
const show = (slice: UsageDay | UsageDaySlice, metric: UsageMetric) =>
  metric === "cost" ? coverageLabel(slice).text : `${formatTokens(slice.tokens)} tokens`;

/** Exact numbers for the day plus who spent them — the per-provider split the
 *  single-series bars can't show. */
function dayTip(d: UsageDay, metric: UsageMetric): string {
  const parts = [`${formatHeatDay(d.day)} · ${show(d, metric)}`];
  if (d.byProvider.length >= 2) {
    parts.push(...d.byProvider.map((p) => `${providerLabel(p.provider)} ${show(p, metric)}`));
  }
  // Why the day's cost is a floor, when it is one.
  const coverage = metric === "cost" ? coverageLabel(d).tip : null;
  if (coverage) parts.push(coverage);
  return parts.join(" · ");
}

/** The right column: one bar per day in range. */
export function UsageChart({
  stats,
  metric,
  loading,
}: {
  stats: UsageStats | null;
  metric: UsageMetric;
  loading: boolean;
}) {
  const daily = stats?.daily ?? [];
  const every = tickEvery(daily.length);
  const bars: MiniBar[] = daily.map((d, i) => ({
    key: d.day,
    value: value(d, metric),
    label: i % every === 0 ? formatDayTick(d.day) : "",
    tip: dayTip(d, metric),
  }));
  // Tokens ran, but none of them on a model the catalog can price — the flat
  // cost chart is about the price table, not about the usage, so say which.
  const unpricedNote =
    stats && stats.totalTokens > 0 && stats.unpricedTokens >= stats.totalTokens
      ? ` Every model that ran (${formatTokens(stats.totalTokens)} tokens) is missing a list price.`
      : "";

  return (
    <div className="usg-chart">
      <div className="usg-chart-t text-sm">
        Daily {metric === "cost" ? "cost" : "processed tokens"}
      </div>
      <MiniBars
        bars={bars}
        loading={loading && !stats}
        ariaLabel={`${metric === "cost" ? "Cost" : "Processed tokens"} per day`}
        // A tokens chart is empty only when nothing ran; a cost chart is also
        // empty when everything that ran used models the catalog can't price.
        empty={
          metric === "cost" ? (
            <>No priced usage in this window.{unpricedNote}</>
          ) : (
            <>Nothing recorded in this window.</>
          )
        }
      />
    </div>
  );
}
