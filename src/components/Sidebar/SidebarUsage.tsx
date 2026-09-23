import { Icon } from "@/components/Icon";
import { Chip, IconButton } from "@/components/ui";
import { costLabel, useUsageStats } from "@/data/usage";
import { useAppStore } from "@/store";

/** The footer's way into the Usage screen: the past 7 days' spend across every
 *  host as a compact chip. Mounting starts the same transcript scan the Usage
 *  screen runs (shared cache, one scan per host, five-minute TTL), so the
 *  figure lands a moment after launch; until then the chip holds a dimmed
 *  placeholder. It falls back to a bare icon when there is no price to show:
 *  the flag is off, the scan failed before ever answering, or nothing that ran
 *  could be priced. */
export function SidebarUsage() {
  const openUsageScreen = useAppStore((s) => s.openUsageScreen);
  const showCost = useAppStore((s) => s.features.sidebarCost);
  const { stats, loading, error, catalogReady } = useUsageStats("7d");

  if (!showCost || (error && !stats)) return <UsageIcon onClick={openUsageScreen} />;

  // Numbers are on the way: the first scan is still running, or it has landed
  // but the price catalog has not, so every bucket would read "Unpriced" for a
  // moment and then flip to a figure.
  if (loading || !stats || !catalogReady) {
    return (
      <Chip className="side-usage pending" tip="Scanning usage…" onClick={openUsageScreen}>
        <Icon name="activity" />
        <span className="mono">$ –</span>
      </Chip>
    );
  }

  const cost = costLabel(stats.totalCostUsd, stats.totalTokens, stats.unpricedTokens);
  if (cost.kind === "unpriced") return <UsageIcon onClick={openUsageScreen} />;

  return (
    <Chip
      className="side-usage"
      tip={cost.tip ? `7-day spend · ${cost.tip}` : "7-day spend"}
      onClick={openUsageScreen}
    >
      <Icon name="activity" />
      <span className="mono">{cost.text}</span>
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
