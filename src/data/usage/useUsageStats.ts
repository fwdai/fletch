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

/** How one host's scan is going. `scan` is the last answer that host gave; it
 *  survives a re-scan and a failure, because stale numbers beat an empty pane,
 *  and it is what separates a re-scan behind numbers on screen from a cold
 *  first load. */
export type HostScanState =
  | { status: "loading"; scan?: HostUsageScan }
  | { status: "ok"; scan: HostUsageScan }
  | { status: "error"; error: string; scan?: HostUsageScan };

/** Every host's scan state, keyed by environment id. The single source the pane
 *  derives from: a host that failed has an entry like any other, so it can be
 *  counted, offered by the filter and explained. */
export type HostScanStates = Record<EnvironmentId, HostScanState>;

/** One host the pane can filter to, in the order the filter shows them. */
export interface UsageHost {
  id: EnvironmentId;
  name: string;
  /** How this host's scan went. Every current target is listed whatever its
   *  status: dropping a failed host is how a header ends up claiming to cover a
   *  machine that contributed nothing. */
  status: HostScanState["status"];
  /** Why this host has no numbers. Null unless `status` is `"error"`. */
  error: string | null;
}

export interface UsageStatsResult {
  /** null until the first scan lands. Survives a failed re-scan — stale numbers
   *  beat an empty pane. */
  stats: UsageStats | null;
  /** Every host the pane is asking, local first, each with its own status. One
   *  entry means the filter has nothing to offer and is hidden. */
  hosts: UsageHost[];
  /** No data on screen yet: render skeletons. */
  loading: boolean;
  /** A re-scan is running behind numbers that are already on screen: keep them,
   *  but dim them so it's visible that they may move. */
  refreshing: boolean;
  /** Set only when every host in the current selection failed. A host that
   *  failed beside others that didn't is not an error for the pane: the numbers
   *  on screen are still true, just of fewer machines, and both the header count
   *  and the filter say which ones. */
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

/** The cached scans for `hosts` as already-settled state, so reopening the pane
 *  shows the last scan instantly instead of skeletons. A host with nothing
 *  cached is left out and `load` puts it in as `"loading"`. */
function seedStates(hosts: readonly EnvironmentEntry[]): HostScanStates {
  const seeded: HostScanStates = {};
  for (const h of hosts) {
    const hit = cache.get(h.id);
    if (hit) seeded[h.id] = { status: "ok", scan: hit };
  }
  return seeded;
}

/** What `deriveUsageView` reads off the scan states: the host list plus the
 *  scans the current selection folds over and the states the pane renders
 *  around them. Everything but the aggregation, which needs the catalog. */
export interface UsageView {
  hosts: UsageHost[];
  /** The scans the selection covers, in the filter's order, each carrying its
   *  host's current display name. */
  scans: HostUsageScan[];
  loading: boolean;
  refreshing: boolean;
  error: string | null;
  scannedAt: number | null;
}

/** Everything the pane shows about the hosts, folded out of the scan states of
 *  the hosts it is asking *right now*.
 *
 *  Intersecting with `targets` on every derivation is what keeps a slow host
 *  honest: a scan that settles after its host disconnected writes its own entry
 *  and changes nothing on screen, because a host that is no longer a target is
 *  no longer read. No generation counter, and no way for a stale batch to
 *  re-add a departed host's numbers under a header that has stopped counting
 *  it.
 *
 *  `host` is an environment id, or `ALL_HOSTS` for every target. */
export function deriveUsageView(
  targets: readonly EnvironmentEntry[],
  states: HostScanStates,
  host: string = ALL_HOSTS,
): UsageView {
  // A target with no entry yet (its `load` has not run) reads as loading: it is
  // a host we intend to ask, which is what a skeleton means.
  const hosts: UsageHost[] = targets.map((t) => {
    const state = states[t.id];
    return {
      id: t.id,
      name: t.name,
      status: state?.status ?? "loading",
      error: state?.status === "error" ? state.error : null,
    };
  });

  const scoped = hosts.filter((h) => host === ALL_HOSTS || h.id === host);
  // Renamed hosts pick up the new name without a re-scan.
  const scans = scoped.flatMap((h) => {
    const scan = states[h.id]?.scan;
    return scan ? [{ ...scan, envName: h.name }] : [];
  });
  // Only a selection where every host failed has nothing to say; one failure
  // beside a host that answered leaves the numbers true, of fewer machines.
  const failed = scoped.filter((h) => h.status === "error");
  const error = scoped.length > 0 && failed.length === scoped.length ? failed[0].error : null;
  const scanning = scoped.some((h) => h.status === "loading");

  return {
    hosts,
    scans,
    loading: scans.length === 0 && error === null,
    refreshing: scans.length > 0 && scanning,
    error,
    scannedAt: scans.length > 0 ? Math.min(...scans.map((s) => s.fetchedAt)) : null,
  };
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
 *  Hosts are tracked one by one: each scan settles into its own entry of a
 *  state map, and the pane's view is derived from that map intersected with the
 *  hosts currently worth asking (`deriveUsageView`).
 *
 *  `host` is an environment id, or `ALL_HOSTS` for the merged view. */
export function useUsageStats(range: UsageRange, host: string = ALL_HOSTS): UsageStatsResult {
  const catalog = useAppStore((s) => s.modelCatalog);
  const environments = useAppStore((s) => s.environments);
  const targets = useMemo(() => usageScanHosts(environments), [environments]);

  const [states, setStates] = useState<HostScanStates>(() => seedStates(targets));
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
    const staleIds = new Set(stale.map((t) => t.id));
    // One write for what this load knows: whoever is about to be scanned turns
    // "loading" while keeping the scan it already has, and whoever the cache
    // still answers for is settled from it (a host that just joined the set
    // with a fresh entry included).
    setStates((prev) => {
      const next: HostScanStates = { ...prev };
      for (const t of current) {
        const hit = cache.get(t.id);
        if (staleIds.has(t.id)) next[t.id] = { status: "loading", scan: hit };
        else if (hit) next[t.id] = { status: "ok", scan: hit };
      }
      return next;
    });
    // Each host settles on its own and writes only its own entry, so one host's
    // failure takes nothing off the screen and a slow host answering late
    // cannot overwrite what a later load already settled.
    for (const t of stale) {
      scanHost(t).then(
        () => {
          const hit = cache.get(t.id);
          if (!alive.current || !hit) return;
          setStates((prev) => ({ ...prev, [t.id]: { status: "ok", scan: hit } }));
        },
        (err: unknown) => {
          console.warn(`usage scan failed on ${t.name}`, err);
          if (!alive.current) return;
          setStates((prev) => ({
            ...prev,
            [t.id]: {
              status: "error",
              error: err instanceof Error ? err.message : String(err),
              scan: cache.get(t.id),
            },
          }));
        },
      );
    }
  }, []);

  // Mount, and whenever the scannable host set changes: a newly connected host
  // is scanned, a departed one just stops being asked, and an unchanged set
  // costs one cache read. A range or host switch reslices what's already here.
  useEffect(() => {
    load(targets, false);
  }, [load, targets]);

  const view = useMemo(() => deriveUsageView(targets, states, host), [targets, states, host]);

  const stats = useMemo(() => {
    if (view.scans.length === 0) return null;
    const merged = mergeScans(view.scans);
    // Sub-ranges end where the scans ended, so no range can claim a window
    // they didn't cover and the chart's last column is the last scanned hour.
    return aggregateUsage(merged, catalog, rangeBounds(range, merged.untilMs));
  }, [view.scans, catalog, range]);

  const refresh = useCallback(() => load(targets, true), [load, targets]);

  return {
    stats,
    hosts: view.hosts,
    loading: view.loading,
    refreshing: view.refreshing,
    error: view.error,
    catalogReady: Object.keys(catalog).length > 0,
    refresh,
    scannedAt: view.scannedAt,
  };
}
