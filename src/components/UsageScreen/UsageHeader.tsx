import { Icon } from "@/components/Icon";
import { SetHead, SetSeg } from "@/components/SettingsScreen/primitives";
import { formatDayTick } from "@/components/Stats";
import { IconButton } from "@/components/ui";
import { ALL_HOSTS, type UsageHost, type UsageRange, type UsageRangeBounds } from "@/data/usage";
import { formatAge, formatClockTime, localDay } from "@/util/format";

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
  /** Every host being asked, local first, each with its scan status. One of
   *  them means there is nothing to choose between and the host filter is not
   *  rendered — the same rule the sidebar's environment switcher follows. */
  hosts: UsageHost[];
  /** An environment id, or `ALL_HOSTS`. */
  host: string;
  onHost: (id: string) => void;
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

/** Whose sessions the numbers cover: one machine, one named host, or all of
 *  them. Never claims "this machine" once more than one host is in play, and
 *  counts exactly the hosts whose scans the totals were folded from
 *  (`UsageHost.covered`) — not the hosts whose status happens to look healthy.
 *  A host whose re-scan failed still has its last scan in the numbers and is
 *  still counted; one that has never answered, failed or still scanning, is
 *  not, and the skeletons say why. */
function scopeText(hosts: UsageHost[], host: string): string {
  if (hosts.length < 2) return "on this machine";
  if (host !== ALL_HOSTS) return `on ${hosts.find((h) => h.id === host)?.name ?? "this host"}`;
  const covered = hosts.filter((h) => h.covered).length;
  if (covered === hosts.length) return `across all ${hosts.length} hosts`;
  return `across ${covered} of ${hosts.length} hosts`;
}

/** The pane header. The host filter, the period picker and the re-scan button
 *  sit on the eyebrow row, where the Customize panes keep their section switch:
 *  all three act on the whole page, unlike the Cost / Tokens switch, which only
 *  re-reads the top section and so lives on that section's own header. */
export function UsageHeader({
  range,
  onRange,
  hosts,
  host,
  onHost,
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
      eyebrow="Usage"
      eyebrowAside={
        <div className="usg-actions">
          {hosts.length > 1 && (
            <SetSeg
              value={host}
              options={[
                { value: ALL_HOSTS, label: "All hosts" },
                // A host whose scan failed stays selectable and says why on
                // hover; picking it shows that error where its numbers would
                // be, rather than skeletons that would never resolve. The same
                // `covered` flag the header counts says whether the failure
                // took its numbers away or only its freshness.
                ...hosts.map((h) => ({
                  value: h.id,
                  label: h.name,
                  ...(h.error
                    ? {
                        tip: h.covered
                          ? `Scan failed: ${h.error} (showing its last scan)`
                          : `Scan failed: ${h.error}`,
                      }
                    : {}),
                })),
              ]}
              onChange={onHost}
            />
          )}
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
          Codex session {scopeText(hosts, host)}, read straight from their transcripts.
          {scanned && <span className="usg-scanned mono">{scanned}</span>}
        </>
      }
    />
  );
}
