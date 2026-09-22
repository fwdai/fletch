import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { UsageScan } from "@/api";
import { api } from "@/api";
import { hostSupports } from "@/remote/types";
import { useAppStore } from "@/store";
import type { EnvironmentEntry, EnvironmentId } from "@/store/environments";
import { aggregateUsage, mergeScans, rangeBounds, WIDEST_RANGE } from "./aggregate";
import type { HostUsageScan, UsageRange, UsageStats } from "./types";

/** The op each host answers for its own disk. */
const SCAN_OP = "scan_usage_transcripts";

/** The host filter's "no filter" value. Not an environment id — `"local"` is
 *  one, and every other is a host public key, so no host can collide with it. */
export const ALL_HOSTS = "all";

/** One host the pane can filter to, in the order the filter shows them. */
export interface UsageHost {
  id: EnvironmentId;
  name: string;
}

export interface UsageStatsResult {
  /** null until the first scan lands. Survives a failed re-scan — stale numbers
   *  beat an empty pane. */
  stats: UsageStats | null;
  /** The hosts that contributed (or are contributing) a scan, local first. One
   *  entry means the filter has nothing to offer and is hidden. */
  hosts: UsageHost[];
  /** No data on screen yet: render skeletons. */
  loading: boolean;
  /** A re-scan is running behind numbers that are already on screen: keep them,
   *  but dim them so it's visible that they may move. */
  refreshing: boolean;
  /** Set only when *no* host answered. A host that failed beside others that
   *  didn't is a console warning: the numbers on screen are still true, just of
   *  fewer machines, and the filter shows which ones. */
  error: string | null;
  /** The model catalog has prices in it. False on a cold start before the
   *  catalog lands, when every bucket prices to null and a cost of "$0.00"
   *  would be a statement about the catalog, not about the usage. */
  catalogReady: boolean;
  /** Force a re-scan, ignoring the cache's TTL. */
  refresh: () => void;
  /** When the oldest scan on screen was taken, epoch ms; null before the first
   *  one. The oldest, because that is the age the whole figure inherits. */
  scannedAt: number | null;
}

/** How long a scan stays good enough to show without re-reading the disk. */
export const SCAN_TTL_MS = 5 * 60_000;

/** Is a cached scan still worth showing without revalidating? */
export function isFresh(fetchedAt: number, nowMs: number, ttlMs = SCAN_TTL_MS): boolean {
  return nowMs - fetchedAt < ttlMs;
}

// Module-level so closing and reopening the pane shows the last scan instantly
// instead of replaying a multi-second disk walk. Keyed per environment: hosts
// come and go independently, and one host's answer must never be served as
// another's. `inFlight` collapses the concurrent callers (StrictMode's double
// mount, a refresh during a revalidate) onto one scan per host.
const cache = new Map<EnvironmentId, HostUsageScan>();
const inFlight = new Map<EnvironmentId, Promise<void>>();

/** Which environments to ask. Local always — its transcripts are on this disk
 *  whatever environment the UI is driving. A remote host only when it is
 *  connected *and* says it answers the op: hosts that predate it would reply
 *  "unknown op", which is not a failure worth showing. Local first, then by
 *  name, so the filter's order is stable across reconnects. */
export function usageScanHosts(
  environments: Record<EnvironmentId, EnvironmentEntry>,
): EnvironmentEntry[] {
  return Object.values(environments)
    .filter(
      (e) =>
        e.kind === "local" || (e.connection === "connected" && hostSupports(e.protocol, SCAN_OP)),
    )
    .sort((a, b) =>
      a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "local" ? -1 : 1,
    );
}

/** Scan one host over the widest window, writing the answer into the cache.
 *  Rejects with whatever the transport said; the caller decides what a single
 *  host's failure means. */
function scanHost(env: EnvironmentEntry): Promise<void> {
  const existing = inFlight.get(env.id);
  if (existing) return existing;
  const { sinceMs, untilMs } = rangeBounds(WIDEST_RANGE, Date.now());
  // The local environment carries no transport by design (see
  // store/environments.ts); `api` goes down Tauri IPC to this machine.
  const call = env.transport
    ? env.transport.call<UsageScan>(SCAN_OP, { sinceMs, untilMs })
    : api.scanUsageTranscripts(sinceMs, untilMs);
  const run = call
    .then((scan) => {
      cache.set(env.id, { envId: env.id, envName: env.name, scan, fetchedAt: Date.now() });
    })
    .finally(() => {
      inFlight.delete(env.id);
    });
  inFlight.set(env.id, run);
  return run;
}

/** The cached scans for `hosts`, in their order, skipping hosts that have not
 *  answered yet. Renamed hosts pick up the new name without a re-scan. */
function cached(hosts: readonly EnvironmentEntry[]): HostUsageScan[] {
  return hosts.flatMap((h) => {
    const hit = cache.get(h.id);
    return hit ? [{ ...hit, envName: h.name }] : [];
  });
}

/** Scan every connected host's Claude Code / Codex transcripts once, then
 *  answer every range and every host selection from those scans.
 *
 *  Three costs are kept apart. A scan is disk work measured in seconds, so it
 *  runs for the widest window only, once per host, and is cached across pane
 *  opens (stale-while-revalidate, `SCAN_TTL_MS`). Slicing a range — or a host —
 *  out of the scans in hand is a pure fold, so switching either is a `useMemo`
 *  and nothing else. Pricing depends on a catalog that can refresh under us, so
 *  a catalog update re-prices the scans in hand rather than re-reading disks.
 *
 *  `host` is an environment id, or `ALL_HOSTS` for the merged view. */
export function useUsageStats(range: UsageRange, host: string = ALL_HOSTS): UsageStatsResult {
  const catalog = useAppStore((s) => s.modelCatalog);
  const environments = useAppStore((s) => s.environments);
  const targets = useMemo(() => usageScanHosts(environments), [environments]);
  // The id/name pairs the filter renders.
  const hosts = useMemo<UsageHost[]>(
    () => targets.map((t) => ({ id: t.id, name: t.name })),
    [targets],
  );

  const [scans, setScans] = useState<HostUsageScan[]>(() => cached(targets));
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const load = useCallback((current: readonly EnvironmentEntry[], force: boolean) => {
    const now = Date.now();
    const stale = current.filter((t) => {
      const hit = cache.get(t.id);
      return force || !hit || !isFresh(hit.fetchedAt, now);
    });
    if (stale.length === 0) {
      setScans(cached(current));
      return;
    }
    setScanning(true);
    Promise.all(
      // One host's failure must not take the others' numbers with it, so each
      // scan settles on its own and the rejection becomes a message.
      stale.map((t) =>
        scanHost(t).then(
          () => null,
          (err: unknown) => {
            console.warn(`usage scan failed on ${t.name}`, err);
            return err instanceof Error ? err.message : String(err);
          },
        ),
      ),
    ).then((results) => {
      if (!alive.current) return;
      const failures = results.filter((r): r is string => r !== null);
      setScans(cached(current));
      setScanning(false);
      // Only a total failure is the pane's business. Compared against the whole
      // host set, not just the ones asked: a host that failed while another's
      // fresh scan was served from cache has taken nothing off the screen.
      setError(failures.length === current.length ? failures[0] : null);
    });
  }, []);

  // Mount, and whenever the scannable host set changes: a newly connected host
  // is scanned, a departed one just stops being asked, and an unchanged set
  // costs one cache read. A range or host switch reslices what's already here.
  useEffect(() => {
    load(targets, false);
  }, [load, targets]);

  // The host filter is a filter on the scans in hand, applied before the fold.
  const selected = useMemo(
    () => (host === ALL_HOSTS ? scans : scans.filter((s) => s.envId === host)),
    [scans, host],
  );

  const stats = useMemo(() => {
    if (selected.length === 0) return null;
    const merged = mergeScans(selected);
    // Sub-ranges end where the scans ended, so no range can claim a window
    // they didn't cover and the chart's last column is the last scanned hour.
    return aggregateUsage(merged, catalog, rangeBounds(range, merged.untilMs));
  }, [selected, catalog, range]);

  const scannedAt = selected.length > 0 ? Math.min(...selected.map((s) => s.fetchedAt)) : null;

  const refresh = useCallback(() => load(targets, true), [load, targets]);

  return {
    stats,
    hosts,
    loading: stats === null && error === null,
    refreshing: scanning && stats !== null,
    error,
    catalogReady: Object.keys(catalog).length > 0,
    refresh,
    scannedAt,
  };
}
