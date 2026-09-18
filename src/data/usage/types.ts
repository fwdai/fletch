import type { UsageProvider } from "@/api";

// View model for "how many tokens and dollars did every local coding-agent
// session burn". Derived from a raw transcript scan (`UsageScan`) plus the
// model catalog's list prices — see `aggregate.ts`. Kept free of React so any
// surface (settings pane, a future dashboard, the mobile app) can render it.

/** How far back the usage view looks. */
export type UsageRange = "24h" | "7d" | "30d" | "90d";

/** Which number the headline and the daily chart plot. */
export type UsageMetric = "cost" | "tokens";

/** The window the stats describe, in epoch ms. */
export interface UsageRangeBounds {
  sinceMs: number;
  untilMs: number;
}

/** Token counts summed over some slice of the scan, plus their total. */
export interface UsageTokenTotals {
  /** Fresh (uncached) input. */
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  /** input + output + cacheRead + cacheWrite — the "processed tokens" figure. */
  processed: number;
}

/** Every level that carries dollars carries its pricing coverage alongside
 *  them: `costUsd` is what WAS priced, `unpricedTokens` is how many processed
 *  tokens ran on models the catalog has no rate for. A reader can then be told
 *  "$4.10" or "at least $4.10" or "unpriced" instead of being handed a partial
 *  sum dressed up as a total — see `costLabel`. */
export interface UsageCostCoverage {
  costUsd: number;
  /** Processed tokens whose bucket priced to null. */
  unpricedTokens: number;
}

/** The totals strip: every token bucket plus what the cache saved. */
export interface UsageTotals extends UsageTokenTotals {
  /** List-price dollars avoided by cache reads, over the priced buckets only. */
  cacheSavingsUsd: number;
  /** Cache-read tokens on models with no price — the only part of `cacheRead`
   *  the savings figure can be missing. Read against `cacheRead`, not
   *  `processed`: an unpriced model that never read from cache saved nothing
   *  unknown, whatever else it processed. */
  unpricedCacheReadTokens: number;
}

/** One provider's slice of the window. */
export interface UsageProviderRow extends UsageCostCoverage {
  provider: UsageProvider;
  /** Distinct sessions observed in the window. */
  sessions: number;
  /** Processed tokens. */
  tokens: number;
  /** Fraction of all processed tokens, 0–1. */
  share: number;
}

/** One provider's slice of a single day — the chart's hover breakdown. */
export interface UsageDaySlice extends UsageCostCoverage {
  provider: UsageProvider;
  tokens: number;
}

/** One column of the daily chart. Present for every day in range, zeroed on
 *  days with no activity, so the x-axis is a real calendar rather than a list
 *  of the days that happened to have traffic. */
export interface UsageDay extends UsageCostCoverage {
  /** Local date key, YYYY-MM-DD. */
  day: string;
  tokens: number;
  byProvider: UsageDaySlice[];
}

/** A row of the breakdown table in Model mode. */
export interface UsageModelRow {
  /** Model id as the transcript reported it. */
  model: string;
  provider: UsageProvider;
  tokens: number;
  /** null when the catalog has no price for this model — rendered "Unpriced"
   *  rather than $0.00, which would claim the calls were free. */
  costUsd: number | null;
  /** Fraction of all processed tokens, 0–1. */
  share: number;
}

/** A row of the breakdown table in Day mode. Only days with activity. */
export interface UsageDayRow extends UsageCostCoverage {
  day: string;
  tokens: number;
  share: number;
}

export interface UsageStats {
  /** The window these numbers cover. */
  range: UsageRangeBounds;
  /** Processed tokens across every provider. */
  totalTokens: number;
  /** Summed list-price cost of the buckets that could be priced. Read it with
   *  `unpricedTokens`: alone it is a lower bound, not a total. */
  totalCostUsd: number;
  /** Processed tokens that ran on models the catalog has no rate for. 0 means
   *  `totalCostUsd` is the whole story; `totalTokens` means nothing is priced. */
  unpricedTokens: number;
  totalSessions: number;
  providers: UsageProviderRow[];
  totals: UsageTotals;
  /** Every day in range, oldest first. */
  daily: UsageDay[];
  /** Sorted by cost desc, then tokens desc; unpriced models last. */
  byModel: UsageModelRow[];
  /** Most recent day first. */
  byDay: UsageDayRow[];
  /** Transcript files the scan read — surfaced so an empty result can say
   *  whether it found nothing or looked at nothing. */
  scannedFiles: number;
  /** The scan turned up no usage at all. */
  empty: boolean;
}
