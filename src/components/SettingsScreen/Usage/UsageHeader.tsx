import { Icon } from "@/components/Icon";
import { formatDayTick } from "@/components/Stats";
import { IconButton } from "@/components/ui";
import type { UsageRange, UsageRangeBounds } from "@/data/usage";
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

interface Props {
  range: UsageRange;
  onRange: (r: UsageRange) => void;
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

/** The pane header. The period picker and the re-scan button sit on the eyebrow
 *  row, where the Customize panes keep their section switch: both act on the
 *  whole page, unlike the Cost / Tokens switch, which only re-reads the top
 *  section and so lives on that section's own header. */
export function UsageHeader({ range, onRange, bounds, busy, scannedAt, onRefresh }: Props) {
  // Every range is sliced out of one cached scan, so how old that scan is
  // matters more than which window is selected — say so, quietly.
  const scanned = busy ? "scanning…" : scannedAt === null ? null : scanAge(scannedAt);

  return (
    <SetHead
      eyebrow="Settings · Usage"
      eyebrowAside={
        <div className="usg-actions">
          <SetSeg<UsageRange> value={range} options={RANGES} onChange={onRange} />
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
      title="Usage"
      desc={
        <>
          <span className="usg-range mono">{rangeText(range, bounds)}</span> · every Claude Code and
          Codex session on this machine, read straight from their transcripts.
          {scanned && <span className="usg-scanned mono">{scanned}</span>}
        </>
      }
    />
  );
}
