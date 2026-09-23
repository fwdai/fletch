import { Icon } from "@/components/Icon";
import { Chip, IconButton } from "@/components/ui";
import { costLabel, useUsageStats } from "@/data/usage";
import { useAppStore } from "@/store";
import { formatTokens } from "@/util/format";

/** The footer's way into the Usage screen: the past 7 days' tokens across every
 *  host as a bordered chip with a trailing chevron, so it reads as a control
 *  that goes somewhere rather than a line of stats. Tokens, not dollars: the
 *  cost is a list-price estimate and most people here pay a subscription, so
 *  the figure that is actually true leads and the estimate rides in the tip.
 *
 *  Mounting starts the same transcript scan the Usage screen runs (shared
 *  cache, one scan per host, five-minute TTL), so the number lands a moment
 *  after launch; until then the chip holds a dimmed placeholder. It falls back
 *  to a bare icon when the flag is off or the scan failed before ever
 *  answering. */
export function SidebarUsage() {
  const openUsageScreen = useAppStore((s) => s.openUsageScreen);
  const enabled = useAppStore((s) => s.features.sidebarUsage);
  const { stats, loading, error } = useUsageStats("7d");

  if (!enabled || (error && !stats)) return <UsageIcon onClick={openUsageScreen} />;

  if (loading || !stats) {
    return (
      <Chip bordered className="side-usage pending" tip="Scanning usage…" onClick={openUsageScreen}>
        <Icon name="activity" />
        <span className="mono">–</span>
        <Icon name="chevR" className="side-usage-chev" />
      </Chip>
    );
  }

  // The estimate is a footnote: skipped entirely when nothing could be priced.
  const cost = costLabel(stats.totalCostUsd, stats.totalTokens, stats.unpricedTokens);
  const estimate = cost.kind === "unpriced" ? "" : ` · ≈ ${cost.text} at list API prices`;

  return (
    <Chip
      bordered
      className="side-usage"
      tip={`Tokens in the past 7 days${estimate}`}
      onClick={openUsageScreen}
    >
      <Icon name="activity" />
      <span className="mono">{formatTokens(stats.totalTokens)} tokens</span>
      <Icon name="chevR" className="side-usage-chev" />
    </Chip>
  );
}

function UsageIcon({ onClick }: { onClick: () => void }) {
  return (
    <IconButton tip="Usage" onClick={onClick}>
      <Icon name="activity" />
    </IconButton>
  );
}
