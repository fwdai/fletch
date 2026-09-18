import { formatDayTick, formatHeatDay, type MiniBar, MiniBars } from "@/components/Stats";
import { providerLabel } from "@/data/providers";
import type { UsageDay, UsageMetric, UsageStats } from "@/data/usage";
import { formatCost, formatTokens } from "@/util/format";

/** Roughly six ticks whatever the range, so 24h/7d label every column and 90d
 *  labels every other week. */
const tickEvery = (days: number) => Math.max(1, Math.ceil(days / 6));

const value = (d: UsageDay, metric: UsageMetric) => (metric === "cost" ? d.costUsd : d.tokens);

const show = (n: number, metric: UsageMetric) =>
  metric === "cost" ? formatCost(n) : `${formatTokens(n)} tokens`;

/** Exact numbers for the day plus who spent them — the per-provider split the
 *  single-series bars can't show. */
function dayTip(d: UsageDay, metric: UsageMetric): string {
  const head = `${formatHeatDay(d.day)} · ${show(value(d, metric), metric)}`;
  if (d.byProvider.length < 2) return head;
  const parts = d.byProvider.map(
    (p) => `${providerLabel(p.provider)} ${show(metric === "cost" ? p.costUsd : p.tokens, metric)}`,
  );
  return `${head} · ${parts.join(" · ")}`;
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
            <>No priced usage in this window.</>
          ) : (
            <>Nothing recorded in this window.</>
          )
        }
      />
    </div>
  );
}
