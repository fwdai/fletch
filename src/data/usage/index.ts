// Usage across every coding-agent session this client can see — on this machine
// and on every connected host: scan → merge → aggregate → view model.
// `useUsageStats` is the entry point for UI; `mergeScans` and `aggregateUsage`
// are the pure folds behind it, reusable by any other surface that has scans in
// hand.

export {
  aggregateUsage,
  bucketsInRange,
  localHourStart,
  mergeScans,
  processedTokens,
  rangeBounds,
  sessionsInRange,
  WIDEST_RANGE,
} from "./aggregate";
export {
  type CostKind,
  type CostLabel,
  costLabel,
  coverageLabel,
  modelCostLabel,
} from "./costLabel";
export type {
  HostUsageScan,
  UsageCostCoverage,
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
export {
  ALL_HOSTS,
  deriveUsageView,
  type HostScanState,
  type HostScanStates,
  isFresh,
  SCAN_TTL_MS,
  type UsageHost,
  type UsageStatsResult,
  type UsageView,
  usageScanHosts,
  useUsageStats,
} from "./useUsageStats";
