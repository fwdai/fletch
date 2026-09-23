import { ProviderIcon } from "@/components/ProviderIcon";
import { CountUp, Skeleton } from "@/components/Stats";
import { providerChip, providerLabel } from "@/data/providers";
import {
  type CostLabel,
  costLabel,
  coverageLabel,
  type UsageMetric,
  type UsageStats,
} from "@/data/usage";
import { formatCost, formatPercent, formatTokens } from "@/util/format";

const sessionsLabel = (n: number) => `${n.toLocaleString()} ${n === 1 ? "session" : "sessions"}`;

/** What the headline number is, given how much of it the catalog could price.
 *  The label carries the caveat so the figure itself stays a figure. */
const COST_CAPTION: Record<CostLabel["kind"], string> = {
  exact: "total cost",
  partial: "total cost so far — some models unpriced",
  unpriced: "no priced usage",
};

const headlineCost = (stats: UsageStats): CostLabel =>
  costLabel(stats.totalCostUsd, stats.totalTokens, stats.unpricedTokens);

/** Cost counts in cents so the ramp animates on a whole number and still lands
 *  on the exact dollar figure. Partial coverage keeps the ramp but prefixes the
 *  "at least" marker; no coverage at all has no number to ramp. */
function Headline({ stats, metric }: { stats: UsageStats; metric: UsageMetric }) {
  if (metric === "tokens") return <CountUp value={stats.totalTokens} format={formatTokens} />;
  const cost = headlineCost(stats);
  if (cost.kind === "unpriced") return <span className="usg-headline-unpriced">Unpriced</span>;
  return (
    <>
      {cost.kind === "partial" && <span className="usg-approx">≥&nbsp;</span>}
      <CountUp value={Math.round(cost.usd * 100)} format={(c) => formatCost(c / 100)} />
    </>
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
  const cost = stats && metric === "cost" ? headlineCost(stats) : null;
  const caption = metric === "tokens" ? "processed tokens" : COST_CAPTION[cost?.kind ?? "exact"];

  return (
    <div className="usg-summary">
      <div
        className={`usg-headline mono${cost?.tip ? " tip" : ""}`}
        data-tip={cost?.tip ?? undefined}
      >
        {stats ? <Headline stats={stats} metric={metric} /> : <Skeleton height={40} />}
      </div>
      <div className="usg-headline-l text-sm">
        {caption} · {stats ? sessionsLabel(stats.totalSessions) : "—"}
      </div>

      <div className="usg-provs">
        {!stats &&
          loading &&
          // One placeholder per provider the pane can show, so the column
          // doesn't grow under the reader when the scan lands.
          [0, 1].map((i) => <Skeleton key={i} height={46} className="usg-prov-skel" />)}
        {stats?.providers.map((p) => {
          const row = coverageLabel(p);
          return (
            <div key={p.provider} className="usg-prov flex-center">
              <ProviderIcon slug={p.provider} {...providerChip(p.provider)} size={26} />
              <div className="usg-prov-id">
                <div className="usg-prov-name text-base">{providerLabel(p.provider)}</div>
                <div className="usg-prov-sub text-xs">{sessionsLabel(p.sessions)}</div>
              </div>
              <div className="usg-prov-nums">
                <div className="usg-prov-tok mono text-base">{formatTokens(p.tokens)}</div>
                <div
                  className={`usg-prov-sub text-xs${row.tip ? " tip" : ""}`}
                  data-tip={row.tip ?? undefined}
                >
                  {formatPercent(p.share)} of tokens ·{" "}
                  <span className={row.kind === "unpriced" ? "usg-unpriced" : undefined}>
                    {row.text}
                  </span>
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
