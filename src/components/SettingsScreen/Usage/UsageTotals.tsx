import { Stat } from "@/components/Stats";
import type { UsageStats } from "@/data/usage";
import { formatCost, formatTokens } from "@/util/format";

/** The token ledger behind the headline: what was sent fresh, what came back
 *  from cache, what was written to it, and what the cache saved in dollars. */
export function UsageTotals({ stats, loading }: { stats: UsageStats | null; loading: boolean }) {
  const t = stats?.totals;
  const busy = loading && !stats;

  return (
    <section className="set-group">
      <div className="set-group-h mono text-xs">Totals</div>
      <div className="stat-row usg-totals text-sm">
        <Stat
          label="processed tokens"
          loading={busy}
          tip="Uncached input + cached input + cache writes + output"
        >
          {t && formatTokens(t.processed)}
        </Stat>
        <span className="stat-sep" />
        <Stat label="cached input" loading={busy} tip="Input served from the prompt cache">
          {t && formatTokens(t.cacheRead)}
        </Stat>
        <span className="stat-sep" />
        <Stat label="uncached input" loading={busy} tip="Input billed at the full rate">
          {t && formatTokens(t.input)}
        </Stat>
        <span className="stat-sep" />
        <Stat label="cache writes" loading={busy} tip="Tokens written into the prompt cache">
          {t && formatTokens(t.cacheWrite)}
        </Stat>
        <span className="stat-sep" />
        <Stat label="output" loading={busy}>
          {t && formatTokens(t.output)}
        </Stat>
        <span className="stat-sep" />
        <Stat
          label="cache savings"
          loading={busy}
          tip="List price of the cached input, minus what the cache read cost"
        >
          {t && formatCost(t.cacheSavingsUsd)}
        </Stat>
      </div>
    </section>
  );
}
