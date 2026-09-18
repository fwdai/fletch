// Usage across every local coding-agent session: scan → aggregate → view model.
// `useUsageStats` is the entry point for UI; `aggregateUsage` is the pure fold
// behind it, reusable by any other surface that has a scan in hand.

export {
  aggregateUsage,
  bucketsInRange,
  localHourStart,
  processedTokens,
  rangeBounds,
  sessionsInRange,
  WIDEST_RANGE,
} from "./aggregate";
export type {
  UsageDay,
  UsageDayRow,
  UsageDaySlice,
  UsageMetric,
  UsageModelRow,
  UsageProviderRow,
  UsageRange,
  UsageRangeBounds,
  UsageStats,
  UsageTokenTotals,
  UsageTotals,
} from "./types";
export { isFresh, SCAN_TTL_MS, type UsageStatsResult, useUsageStats } from "./useUsageStats";
