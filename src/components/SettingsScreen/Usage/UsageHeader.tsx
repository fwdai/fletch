import { Icon } from "@/components/Icon";
import { formatDayTick } from "@/components/Stats";
import { IconButton } from "@/components/ui";
import type { UsageMetric, UsageRange, UsageRangeBounds } from "@/data/usage";
import { formatAge, formatClockTime, localDay } from "@/util/format";
import { SetHead, SetSeg } from "../primitives";

const RANGES: { value: UsageRange; label: string; tip?: string }[] = [
  // Buckets are hourly, so "24h" opens on the hour containing 24 hours ago —
  // at least the past 24 hours, up to an hour more. The tooltip says so, and
  // `rangeText` names the opening hour.
  {
    value: "24h",
    label: "Past 24h",
    tip: "Since this hour yesterday — hourly buckets, so at least the past 24 hours",
  },
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
  { value: "90d", label: "90 days" },
];

const METRICS: { value: UsageMetric; label: string }[] = [
  { value: "cost", label: "Cost" },
  { value: "tokens", label: "Tokens" },
];

interface Props {
  range: UsageRange;
  onRange: (r: UsageRange) => void;
  metric: UsageMetric;
  onMetric: (m: UsageMetric) => void;
  bounds: UsageRangeBounds;
  /** A scan is running — first load or re-scan. Spins the refresh button. */
  busy: boolean;
  /** When the numbers on screen were read from disk, epoch ms. */
  scannedAt: number | null;
  onRefresh: () => void;
}

/** "scanned 4m ago", or "scanned just now" for the first minute. */
function scanAge(at: number): string {
  const age = formatAge(at, Date.now());
  return age === "now" ? "scanned just now" : `scanned ${age} ago`;
}

/** "Aug 19 to Sep 17" — the window in the same tick form the chart axis uses.
 *  The 24h window is hours rather than days, and saying only its dates would
 *  read as two whole days, so it says exactly when it opens: "Since Sep 16,
 *  15:00". */
function rangeText(range: UsageRange, { sinceMs, untilMs }: UsageRangeBounds): string {
  const from = formatDayTick(localDay(sinceMs));
  if (range === "24h") return `Since ${from}, ${formatClockTime(sinceMs)}`;
  return `${from} to ${formatDayTick(localDay(untilMs))}`;
}

export function UsageHeader({
  range,
  onRange,
  metric,
  onMetric,
  bounds,
  busy,
  scannedAt,
  onRefresh,
}: Props) {
  // Every range is sliced out of one cached scan, so how old that scan is
  // matters more than which window is selected — say so, quietly.
  const scanned = busy ? "scanning…" : scannedAt === null ? null : scanAge(scannedAt);

  return (
    <SetHead
      eyebrow="Settings · Usage"
      title="Usage"
      desc={
        <>
          <span className="usg-range mono">{rangeText(range, bounds)}</span> · every Claude Code and
          Codex session on this machine, read straight from their transcripts.
          {scanned && <span className="usg-scanned mono">{scanned}</span>}
        </>
      }
      actions={
        <div className="usg-actions">
          <SetSeg<UsageRange> value={range} options={RANGES} onChange={onRange} />
          <SetSeg<UsageMetric> value={metric} options={METRICS} onChange={onMetric} />
          <IconButton
            size="sm"
            variant="outline"
            tip="Re-scan transcripts"
            aria-label="Refresh usage"
            className={busy ? "usg-refresh busy" : "usg-refresh"}
            onClick={onRefresh}
            disabled={busy}
          >
            <Icon name="refresh" size={13} />
          </IconButton>
        </div>
      }
    />
  );
}
