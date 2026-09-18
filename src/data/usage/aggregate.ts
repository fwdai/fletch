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
import { localDay } from "@/util/format";
import type {
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

const RANGE_DAYS: Record<UsageRange, number> = { "24h": 1, "7d": 7, "30d": 30, "90d": 90 };

/** The widest window any range can ask for. `useUsageStats` scans this once and
 *  slices every narrower range out of it in memory. */
export const WIDEST_RANGE: UsageRange = "90d";

/** How pricing is looked up. Defaults to the model catalog's own helpers;
 *  injectable so the aggregation can be unit-tested without depending on what
 *  models.dev happens to price today. */
export interface UsagePricing {
  priceTokens: (
    catalog: SlimCatalog,
    modelId: string | undefined,
    tokens: UsageScanTokens,
  ) => number | null;
  cacheSavingsUsd: (
    catalog: SlimCatalog,
    modelId: string | undefined,
    tokens: UsageScanTokens,
  ) => number | null;
}

/** input + output + cache reads + cache writes — every token the provider
 *  touched, which is the figure their own usage pages headline. The scan's
 *  token shape is the adapters' `TokenCounts`, so this is the same sum. */
export const processedTokens: (t: UsageScanTokens) => number = totalTokens;

/** The window a range control selects, ending now. Multi-day ranges start at
 *  local midnight N−1 days back, so "7 days" is seven whole calendar columns
 *  on the chart rather than six and a fraction. "24h" is a rolling day. */
export function rangeBounds(range: UsageRange, nowMs: number): UsageRangeBounds {
  if (range === "24h") return { sinceMs: nowMs - DAY_MS, untilMs: nowMs };
  // Stepped from noon so a DST boundary can't land the start on the wrong
  // date, then snapped back to that date's midnight.
  const start = new Date(nowMs);
  start.setHours(12, 0, 0, 0);
  start.setTime(start.getTime() - (RANGE_DAYS[range] - 1) * DAY_MS);
  start.setHours(0, 0, 0, 0);
  return { sinceMs: start.getTime(), untilMs: nowMs };
}

/** The start of the local hour containing `ms`. Buckets are keyed by local hour
 *  start, so a range whose `sinceMs` falls mid-hour must admit the hour it
 *  lands in — otherwise a "24h" window silently drops its oldest bucket. */
export function localHourStart(ms: number): number {
  const d = new Date(ms);
  d.setMinutes(0, 0, 0);
  return d.getTime();
}

/** The buckets a range covers: hour starts from the range's opening hour up to
 *  (but not including) `untilMs`. */
export function bucketsInRange(
  buckets: readonly UsageBucket[],
  { sinceMs, untilMs }: UsageRangeBounds,
): UsageBucket[] {
  const from = localHourStart(sinceMs);
  return buckets.filter((b) => b.hourStartMs >= from && b.hourStartMs < untilMs);
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

/** Every local day key in the window, inclusive, oldest first. */
export function daysInRange({ sinceMs, untilMs }: UsageRangeBounds): string[] {
  const noon = (ms: number) => {
    const d = new Date(ms);
    d.setHours(12, 0, 0, 0);
    return d.getTime();
  };
  const end = noon(untilMs);
  const days: string[] = [];
  for (let t = noon(sinceMs); t <= end; t += DAY_MS) days.push(localDay(t));
  return days;
}

interface DayAcc {
  tokens: number;
  costUsd: number;
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
  pricing?: UsagePricing,
): UsageStats {
  const price = pricing ?? { priceTokens, cacheSavingsUsd };
  const buckets = bucketsInRange(scan.buckets, range);
  const sessions = sessionsInRange(scan.sessions, range);

  const totals = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, processed: 0 };
  let totalCostUsd = 0;
  let savingsUsd = 0;

  const providers = new Map<UsageProvider, { tokens: number; costUsd: number }>();
  const models = new Map<string, UsageModelRow>();
  const days = new Map<string, DayAcc>();

  for (const b of buckets) {
    const dayKey = localDay(b.hourStartMs);
    const tokens = processedTokens(b.tokens);
    const cost = price.priceTokens(catalog, b.model, b.tokens);
    savingsUsd += price.cacheSavingsUsd(catalog, b.model, b.tokens) ?? 0;

    totals.input += b.tokens.input;
    totals.output += b.tokens.output;
    totals.cacheRead += b.tokens.cacheRead;
    totals.cacheWrite += b.tokens.cacheWrite;
    totals.processed += tokens;
    totalCostUsd += cost ?? 0;

    const prov = providers.get(b.provider) ?? { tokens: 0, costUsd: 0 };
    prov.tokens += tokens;
    prov.costUsd += cost ?? 0;
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
      requests: 0,
    };
    row.tokens += tokens;
    row.requests += b.requests;
    // One unpriced bucket makes the whole row unpriced — a partial sum would
    // understate the model's cost while looking like a complete one.
    row.costUsd = cost === null || row.costUsd === null ? null : row.costUsd + cost;
    models.set(modelKey, row);

    const day = days.get(dayKey) ?? { tokens: 0, costUsd: 0, byProvider: new Map() };
    day.tokens += tokens;
    day.costUsd += cost ?? 0;
    const slice = day.byProvider.get(b.provider) ?? {
      provider: b.provider,
      tokens: 0,
      costUsd: 0,
    };
    slice.tokens += tokens;
    slice.costUsd += cost ?? 0;
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
        share: share(tokens),
      };
    })
    .sort((a, b) => b.tokens - a.tokens || b.sessions - a.sessions);

  const daily: UsageDay[] = daysInRange(range).map((day) => {
    const acc = days.get(day);
    return {
      day,
      tokens: acc?.tokens ?? 0,
      costUsd: acc?.costUsd ?? 0,
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
      share: share(acc.tokens),
    }))
    .sort((a, b) => (a.day < b.day ? 1 : a.day > b.day ? -1 : 0));

  return {
    range,
    totalTokens: totals.processed,
    totalCostUsd,
    totalSessions: sessions.length,
    providers: providerRows,
    totals: { ...totals, cacheSavingsUsd: savingsUsd },
    daily,
    byModel,
    byDay,
    scannedFiles: scan.scannedFiles,
    empty: buckets.length === 0,
  };
}
