import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { UsageScan } from "@/api";
import { api } from "@/api";
import { useAppStore } from "@/store";
import { aggregateUsage, rangeBounds, WIDEST_RANGE } from "./aggregate";
import type { UsageRange, UsageStats } from "./types";

export interface UsageStatsResult {
  /** null until the first scan lands. Survives a failed re-scan — stale numbers
   *  beat an empty pane. */
  stats: UsageStats | null;
  /** No data on screen yet: render skeletons. */
  loading: boolean;
  /** A re-scan is running behind numbers that are already on screen: keep them,
   *  but dim them so it's visible that they may move. */
  refreshing: boolean;
  error: string | null;
  /** Force a re-scan, ignoring the cache's TTL. */
  refresh: () => void;
  /** When the scan on screen was taken, epoch ms; null before the first one. */
  scannedAt: number | null;
}

/** How long a scan stays good enough to show without re-reading the disk. */
export const SCAN_TTL_MS = 5 * 60_000;

/** Is a cached scan still worth showing without revalidating? */
export function isFresh(fetchedAt: number, nowMs: number, ttlMs = SCAN_TTL_MS): boolean {
  return nowMs - fetchedAt < ttlMs;
}

interface CachedScan {
  /** Always the widest range; `scan.sinceMs`/`untilMs` echo that window. */
  scan: UsageScan;
  fetchedAt: number;
}

// Module-level so closing and reopening the pane shows the last scan instantly
// instead of replaying a multi-second disk walk. `inFlight` collapses the
// concurrent callers (StrictMode's double mount, a refresh during a revalidate)
// onto one scan.
let cache: CachedScan | null = null;
let inFlight: Promise<CachedScan> | null = null;

function scanWidest(): Promise<CachedScan> {
  if (inFlight) return inFlight;
  const bounds = rangeBounds(WIDEST_RANGE, Date.now());
  const run = api
    .scanUsageTranscripts(bounds.sinceMs, bounds.untilMs)
    .then((scan) => {
      const entry: CachedScan = { scan, fetchedAt: Date.now() };
      cache = entry;
      return entry;
    })
    .finally(() => {
      inFlight = null;
    });
  inFlight = run;
  return run;
}

/** Scan every local Claude Code / Codex transcript once, then answer every
 *  range from that one scan.
 *
 *  Three costs are kept apart. The scan is disk work measured in seconds, so it
 *  runs for the widest window only and is cached across pane opens
 *  (stale-while-revalidate, `SCAN_TTL_MS`). Slicing a range out of it is a pure
 *  fold, so switching ranges is a `useMemo` and nothing else. Pricing depends
 *  on a catalog that can refresh under us, so a catalog update re-prices the
 *  scan in hand rather than re-reading the disk. */
export function useUsageStats(range: UsageRange): UsageStatsResult {
  const catalog = useAppStore((s) => s.modelCatalog);
  const [entry, setEntry] = useState<CachedScan | null>(() => cache);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const load = useCallback((force: boolean) => {
    if (!force && cache && isFresh(cache.fetchedAt, Date.now())) {
      setEntry(cache);
      return;
    }
    setScanning(true);
    setError(null);
    scanWidest()
      .then((next) => {
        if (!alive.current) return;
        setEntry(next);
        setScanning(false);
      })
      .catch((err: unknown) => {
        if (!alive.current) return;
        console.error("usage scan failed", err);
        // The last good scan stays on screen; the error rides alongside it.
        setError(err instanceof Error ? err.message : String(err));
        setScanning(false);
      });
  }, []);

  // Mount only: a range switch reslices what's already here.
  useEffect(() => {
    load(false);
  }, [load]);

  const stats = useMemo(() => {
    if (!entry) return null;
    // Sub-ranges end where the scan ended, so no range can claim a window the
    // scan didn't cover and the chart's last column is the scan's last hour.
    return aggregateUsage(entry.scan, catalog, rangeBounds(range, entry.scan.untilMs));
  }, [entry, catalog, range]);

  const refresh = useCallback(() => load(true), [load]);

  return {
    stats,
    loading: stats === null && error === null,
    refreshing: scanning && stats !== null,
    error,
    refresh,
    scannedAt: entry?.fetchedAt ?? null,
  };
}
