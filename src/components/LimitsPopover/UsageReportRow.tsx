import { Icon } from "@/components/Icon";
import { costLabel, useUsageStats } from "@/data/usage";
import { useAppStore } from "@/store";
import { formatTokens } from "@/util/format";

/** The popover's way on to the Usage screen, carrying the past 7 days' tokens
 *  across every host. Tokens, not dollars: the cost is a list-price estimate
 *  and most people here pay a subscription, so the figure that is actually
 *  true leads and the estimate rides in the tip.
 *
 *  Mounting starts the same transcript scan the Usage screen runs (shared
 *  cache, one scan per host, five-minute TTL), so the first open of a session
 *  waits a moment for its number; a scan that failed leaves the row without
 *  one. */
export function UsageReportRow() {
  const openUsageScreen = useAppStore((s) => s.openUsageScreen);
  const { stats, loading, error } = useUsageStats("7d");

  const cost = stats && costLabel(stats.totalCostUsd, stats.totalTokens, stats.unpricedTokens);
  const tip = cost && cost.kind !== "unpriced" ? `≈ ${cost.text} at list API prices` : undefined;

  return (
    <button
      type="button"
      className={`lim-report flex-center text-sm${tip ? " tip" : ""}`}
      data-tip={tip}
      onClick={openUsageScreen}
    >
      <Icon name="chart" size={13} />
      {stats ? (
        <span className="lim-report-fig">
          <span className="mono">{formatTokens(stats.totalTokens)}</span> tokens
          <span className="lim-report-span"> · 7 days</span>
        </span>
      ) : (
        loading && !error && <span className="lim-report-span">Scanning usage…</span>
      )}
      <span className="lim-report-go">Usage report</span>
      <Icon name="chevR" size={12} />
    </button>
  );
}
