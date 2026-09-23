import { Icon } from "@/components/Icon";
import { Chip, IconButton } from "@/components/ui";
import { costLabel, useCachedUsageStats } from "@/data/usage";
import { useAppStore } from "@/store";

/** The footer's way into the Usage screen: the past 7 days' spend across every
 *  host as a compact chip, or a bare icon when there is no price to show — the
 *  flag is off, nothing has been scanned yet this session, or nothing that ran
 *  could be priced. Reads only what a scan already cached: the footer mounts on
 *  every launch and must not start a transcript walk just by rendering. */
export function SidebarUsage() {
  const openUsageScreen = useAppStore((s) => s.openUsageScreen);
  const showCost = useAppStore((s) => s.features.sidebarCost);
  const stats = useCachedUsageStats("7d");

  const cost =
    showCost && stats
      ? costLabel(stats.totalCostUsd, stats.totalTokens, stats.unpricedTokens)
      : null;

  if (!cost || cost.kind === "unpriced") {
    return (
      <IconButton tip="Usage" onClick={openUsageScreen}>
        <Icon name="activity" />
      </IconButton>
    );
  }

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
