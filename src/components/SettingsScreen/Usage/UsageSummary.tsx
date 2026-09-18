import { ProviderIcon } from "@/components/ProviderIcon";
import { CountUp, Skeleton } from "@/components/Stats";
import { providerChip, providerLabel } from "@/data/providers";
import type { UsageMetric, UsageStats } from "@/data/usage";
import { formatCost, formatPercent, formatTokens } from "@/util/format";

const sessionsLabel = (n: number) => `${n.toLocaleString()} ${n === 1 ? "session" : "sessions"}`;

/** Cost counts in cents so the ramp animates on a whole number and still lands
 *  on the exact dollar figure. */
function Headline({ stats, metric }: { stats: UsageStats; metric: UsageMetric }) {
  if (metric === "tokens") return <CountUp value={stats.totalTokens} format={formatTokens} />;
  return (
    <CountUp value={Math.round(stats.totalCostUsd * 100)} format={(c) => formatCost(c / 100)} />
  );
}

/** The left column: one headline number for the whole window, then the same
 *  number split per provider. */
export function UsageSummary({
  stats,
  metric,
  loading,
}: {
  stats: UsageStats | null;
  metric: UsageMetric;
  loading: boolean;
}) {
  return (
    <div className="usg-summary">
      <div className="usg-headline mono">
        {stats ? <Headline stats={stats} metric={metric} /> : <Skeleton height={40} />}
      </div>
      <div className="usg-headline-l text-sm">
        {metric === "tokens" ? "processed tokens" : "total cost"} ·{" "}
        {stats ? sessionsLabel(stats.totalSessions) : "—"}
      </div>

      <div className="usg-provs">
        {!stats &&
          loading &&
          // One placeholder per provider the pane can show, so the column
          // doesn't grow under the reader when the scan lands.
          [0, 1].map((i) => <Skeleton key={i} height={46} className="usg-prov-skel" />)}
        {stats?.providers.map((p) => (
          <div key={p.provider} className="usg-prov flex-center">
            <ProviderIcon slug={p.provider} {...providerChip(p.provider)} size={26} />
            <div className="usg-prov-id">
              <div className="usg-prov-name text-base">{providerLabel(p.provider)}</div>
              <div className="usg-prov-sub text-xs">{sessionsLabel(p.sessions)}</div>
            </div>
            <div className="usg-prov-nums">
              <div className="usg-prov-tok mono text-base">{formatTokens(p.tokens)}</div>
              <div className="usg-prov-sub text-xs">
                {formatPercent(p.share)} of tokens · {formatCost(p.costUsd)}
              </div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
