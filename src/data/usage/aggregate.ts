import { totalTokens } from "@/adapters/usage";
import type {
  UsageBucket,
  UsageProvider,
  UsageScan,
  UsageScanTokens,
  UsageSessionSpan,
} from "@/api";
import type { SlimCatalog } from "@/data/modelCatalog";
import { cacheSavingsUsd, priceTokens } from "@/data/modelCatalog";
import { dayKeysBetween, localDay } from "@/util/format";
import type {
  HostUsageScan,
  UsageDay,
  UsageDayRow,
  UsageDaySlice,
  UsageModelRow,
  UsageProviderRow,
  UsageRange,
  UsageRangeBounds,
  UsageStats,
} from "./types";

// Pure aggregation from a raw transcript scan to the usage view model. No
// React, no IO — `useUsageStats` supplies the scan and the catalog.

const DAY_MS = 86_400_000;
const HOUR_MS = 3_600_000;

const RANGE_DAYS: Record<UsageRange, number> = { "24h": 1, "7d": 7, "30d": 30, "90d": 90 };

/** The widest window any range can ask for. `useUsageStats` scans this once and
 *  slices every narrower range out of it in memory. */
export const WIDEST_RANGE: UsageRange = "90d";

/** input + output + cache reads + cache writes — every token the provider
 *  touched, which is the figure their own usage pages headline. The scan's
 *  token shape is the adapters' `TokenCounts`, so this is the same sum. */
export const processedTokens: (t: UsageScanTokens) => number = totalTokens;

/** The start of the local hour containing `ms`. The scan buckets by local hour,
 *  so every range boundary is stated in those terms rather than in a precision
 *  the data doesn't have. */
export function localHourStart(ms: number): number {
  const d = new Date(ms);
  d.setMinutes(0, 0, 0);
  return d.getTime();
}

/** The window a range control selects, ending now.
 *
 *  Multi-day ranges start at local midnight N−1 days back, so "7 days" is seven
 *  whole calendar columns on the chart rather than six and a fraction. "24h"
 *  opens on the hour that contains the instant 24 hours ago: buckets are
 *  hourly, so an exact rolling day is not expressible, and the choice is
 *  between dropping up to an hour of the oldest usage or admitting up to an
 *  hour more. It admits more — "past 24h" must never miss usage inside the
 *  past 24 hours — and the header names the opening hour so the reader can
 *  see exactly what the window is. */
export function rangeBounds(range: UsageRange, nowMs: number): UsageRangeBounds {
  if (range === "24h") return { sinceMs: localHourStart(nowMs - 24 * HOUR_MS), untilMs: nowMs };
  // Stepped from noon so a DST boundary can't land the start on the wrong
  // date, then snapped back to that date's midnight.
  const start = new Date(nowMs);
  start.setHours(12, 0, 0, 0);
  start.setTime(start.getTime() - (RANGE_DAYS[range] - 1) * DAY_MS);
  start.setHours(0, 0, 0, 0);
  return { sinceMs: start.getTime(), untilMs: nowMs };
}

/** A range narrowed to the window a scan actually covers.
 *
 *  `rangeBounds` describes the window the *control* selects, which is a claim
 *  about the calendar rather than about the data: a merged scan opens at the
 *  latest host's start (`mergeScans` intersects them), so hosts cached either
 *  side of local midnight leave the selected range opening before any all-host
 *  data exists. Clamping here keeps the day axis and the header's "from" label
 *  describing the window the numbers came from instead of padding it with days
 *  no host was asked about. */
export function clampToScan(bounds: UsageRangeBounds, scan: UsageScan): UsageRangeBounds {
  return {
    sinceMs: Math.max(bounds.sinceMs, scan.sinceMs),
    untilMs: Math.min(bounds.untilMs, scan.untilMs),
  };
}

/** The buckets a range covers: `[sinceMs, untilMs)` by bucket start. Every
 *  bound `rangeBounds` produces is already on an hour (multi-day ranges open at
 *  local midnight, "24h" at a whole hour), so a bucket is in or out with no
 *  rounding — and a caller passing a mid-hour `sinceMs` gets the honest answer
 *  rather than a silently widened window. */
export function bucketsInRange(
  buckets: readonly UsageBucket[],
  { sinceMs, untilMs }: UsageRangeBounds,
): UsageBucket[] {
  return buckets.filter((b) => b.hourStartMs >= sinceMs && b.hourStartMs < untilMs);
}

/** Sessions whose span overlaps the range at all — a session that opened before
 *  the window and was still running inside it counts, which is what "sessions
 *  in the last 7 days" means to a reader. */
export function sessionsInRange(
  sessions: readonly UsageSessionSpan[],
  { sinceMs, untilMs }: UsageRangeBounds,
): UsageSessionSpan[] {
  return sessions.filter((s) => s.lastMs >= sinceMs && s.firstMs < untilMs);
}

/** Fold several hosts' scans into the one `UsageScan` shape `aggregateUsage`
 *  takes, so nothing downstream has to know how many machines answered.
 *
 *  Two things need care. Session ids are unique within a host only — two hosts
 *  can both have a session `abc` — so every id is prefixed with its environment
 *  id, which keeps `totalSessions` an honest count. And the hosts have
 *  independent clocks: the merged window is the *intersection* of theirs
 *  (`max` of the starts, `min` of the ends), so no range sliced out of it can
 *  claim a window some contributing host had not yet reached. Data outside that
 *  intersection is dropped rather than carried — a scan that crossed local
 *  midnight, or hosts cached minutes apart, would otherwise leave the edge
 *  buckets holding only some hosts' usage while every reader (the chart's first
 *  column, `rangeBounds` off `untilMs`) treats them as an all-host total.
 *
 *  Buckets inside the window are concatenated as they are. Two hosts can hold
 *  the same hour/provider/model cell, and `aggregateUsage` sums cells rather
 *  than assuming one per key, so there is nothing to combine here — only to
 *  order, which keeps the result the sorted shape a `UsageScan` claims to be. */
export function mergeScans(scans: readonly HostUsageScan[]): UsageScan {
  // One host is the overwhelmingly common case, and its own scan already is
  // the merge — no ids can collide and no clocks can disagree.
  if (scans.length === 1) return scans[0].scan;

  // The part of the timeline every contributing host actually covered.
  const window: UsageRangeBounds = {
    sinceMs: Math.max(...scans.map((h) => h.scan.sinceMs)),
    untilMs: Math.min(...scans.map((h) => h.scan.untilMs)),
  };

  const buckets = bucketsInRange(
    scans.flatMap((h) => h.scan.buckets),
    window,
  ).sort(
    (a, b) =>
      a.hourStartMs - b.hourStartMs ||
      a.provider.localeCompare(b.provider) ||
      a.model.localeCompare(b.model),
  );
  const sessions = sessionsInRange(
    scans.flatMap((h) => h.scan.sessions.map((s) => ({ ...s, id: `${h.envId}:${s.id}` }))),
    window,
  ).sort(
    (a, b) =>
      a.provider.localeCompare(b.provider) || a.firstMs - b.firstMs || (a.id < b.id ? -1 : 1),
  );
  const sum = (pick: (s: UsageScan) => number) =>
    scans.reduce((total, h) => total + pick(h.scan), 0);

  return {
    buckets,
    sessions,
    scannedFiles: sum((s) => s.scannedFiles),
    filesRead: sum((s) => s.filesRead),
    bytesRead: sum((s) => s.bytesRead),
    ...window,
  };
}

interface DayAcc {
  tokens: number;
  costUsd: number;
  unpricedTokens: number;
  byProvider: Map<UsageProvider, UsageDaySlice>;
}

const byTokensDesc = (a: { tokens: number }, b: { tokens: number }) => b.tokens - a.tokens;

/** Fold a scan into the numbers the usage pane renders: totals, per-provider
 *  rows, a zero-filled daily series, and the model / day breakdowns.
 *
 *  `range` is a *slice* of the scan, not a description of it: the caller scans
 *  the widest window once and re-folds it per range, so switching ranges is a
 *  recompute rather than another trip to disk. */
export function aggregateUsage(
  scan: UsageScan,
  catalog: SlimCatalog,
  range: UsageRangeBounds,
): UsageStats {
  const buckets = bucketsInRange(scan.buckets, range);
  const sessions = sessionsInRange(scan.sessions, range);

  const totals = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, processed: 0 };
  let totalCostUsd = 0;
  // Processed tokens the catalog had no rate for. Carried beside every dollar
  // figure so the UI can say "at least $X" instead of passing a partial sum off
  // as a total — see `costLabel`.
  let unpricedTokens = 0;
  let savingsUsd = 0;
  // Cache reads on unpriced models — the only tokens a savings figure can be
  // missing. An unpriced model that read nothing from cache leaves the savings
  // exact, however many tokens it otherwise processed.
  let unpricedCacheReadTokens = 0;

  const providers = new Map<
    UsageProvider,
    { tokens: number; costUsd: number; unpricedTokens: number }
  >();
  const models = new Map<string, UsageModelRow>();
  const days = new Map<string, DayAcc>();

  for (const b of buckets) {
    const dayKey = localDay(b.hourStartMs);
    const tokens = processedTokens(b.tokens);
    const cost = priceTokens(catalog, b.model, b.tokens);
    savingsUsd += cacheSavingsUsd(catalog, b.model, b.tokens) ?? 0;

    totals.input += b.tokens.input;
    totals.output += b.tokens.output;
    totals.cacheRead += b.tokens.cacheRead;
    totals.cacheWrite += b.tokens.cacheWrite;
    totals.processed += tokens;
    totalCostUsd += cost ?? 0;
    const unpriced = cost === null ? tokens : 0;
    unpricedTokens += unpriced;
    if (cost === null) unpricedCacheReadTokens += b.tokens.cacheRead;

    const prov = providers.get(b.provider) ?? { tokens: 0, costUsd: 0, unpricedTokens: 0 };
    prov.tokens += tokens;
    prov.costUsd += cost ?? 0;
    prov.unpricedTokens += unpriced;
    providers.set(b.provider, prov);

    // Keyed by provider too: the same model id can be reached through more
    // than one CLI, and the row carries a provider icon.
    const modelKey = `${b.provider}::${b.model}`;
    const row = models.get(modelKey) ?? {
      model: b.model,
      provider: b.provider,
      tokens: 0,
      costUsd: 0 as number | null,
      share: 0,
    };
    row.tokens += tokens;
    // One unpriced bucket makes the whole row unpriced — a partial sum would
    // understate the model's cost while looking like a complete one.
    row.costUsd = cost === null || row.costUsd === null ? null : row.costUsd + cost;
    models.set(modelKey, row);

    const day = days.get(dayKey) ?? {
      tokens: 0,
      costUsd: 0,
      unpricedTokens: 0,
      byProvider: new Map(),
    };
    day.tokens += tokens;
    day.costUsd += cost ?? 0;
    day.unpricedTokens += unpriced;
    const slice = day.byProvider.get(b.provider) ?? {
      provider: b.provider,
      tokens: 0,
      costUsd: 0,
      unpricedTokens: 0,
    };
    slice.tokens += tokens;
    slice.costUsd += cost ?? 0;
    slice.unpricedTokens += unpriced;
    day.byProvider.set(b.provider, slice);
    days.set(dayKey, day);
  }

  const share = (tokens: number) => (totals.processed > 0 ? tokens / totals.processed : 0);

  const sessionsBy = new Map<UsageProvider, number>();
  for (const s of sessions) sessionsBy.set(s.provider, (sessionsBy.get(s.provider) ?? 0) + 1);

  const providerRows: UsageProviderRow[] = [...new Set([...providers.keys(), ...sessionsBy.keys()])]
    .map((provider) => {
      const p = providers.get(provider);
      const tokens = p?.tokens ?? 0;
      return {
        provider,
        sessions: sessionsBy.get(provider) ?? 0,
        tokens,
        costUsd: p?.costUsd ?? 0,
        unpricedTokens: p?.unpricedTokens ?? 0,
        share: share(tokens),
      };
    })
    .sort((a, b) => b.tokens - a.tokens || b.sessions - a.sessions);

  const daily: UsageDay[] = dayKeysBetween(range.sinceMs, range.untilMs).map((day) => {
    const acc = days.get(day);
    return {
      day,
      tokens: acc?.tokens ?? 0,
      costUsd: acc?.costUsd ?? 0,
      unpricedTokens: acc?.unpricedTokens ?? 0,
      byProvider: acc ? [...acc.byProvider.values()].sort(byTokensDesc) : [],
    };
  });

  const byModel = [...models.values()]
    .map((m) => ({ ...m, share: share(m.tokens) }))
    // Unpriced models can't be ranked by cost, so they fall to the bottom and
    // order among themselves by tokens.
    .sort((a, b) => (b.costUsd ?? -1) - (a.costUsd ?? -1) || b.tokens - a.tokens);

  const byDay: UsageDayRow[] = [...days.entries()]
    .map(([day, acc]) => ({
      day,
      tokens: acc.tokens,
      costUsd: acc.costUsd,
      unpricedTokens: acc.unpricedTokens,
      share: share(acc.tokens),
    }))
    .sort((a, b) => (a.day < b.day ? 1 : a.day > b.day ? -1 : 0));

  return {
    range,
    totalTokens: totals.processed,
    totalCostUsd,
    unpricedTokens,
    totalSessions: sessions.length,
    providers: providerRows,
    totals: { ...totals, cacheSavingsUsd: savingsUsd, unpricedCacheReadTokens },
    daily,
    byModel,
    byDay,
    scannedFiles: scan.scannedFiles,
    empty: buckets.length === 0,
  };
}
