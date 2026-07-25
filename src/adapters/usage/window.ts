// Resolving the window a context fill is measured against.
//
// Three sources in descending order of authority: the size the agent reported
// for the deployment it's actually talking to (codex does), the catalog entry
// for the model that produced the turn (claude/opencode/pi report a model but
// no size), then a default. Shared by both gauges — two of them disagreeing
// about the denominator is the same class of bug as disagreeing about the
// numerator.

import { lookupModel } from "@/data/modelCatalog/normalize";
import type { SlimCatalog } from "@/data/modelCatalog/types";
import type { UsageSnapshot } from "./index";

/** Fallback for a model the catalog doesn't know; the agents Fletch runs are
 *  200k-class, and codex reports its own so never reaches this. */
export const DEFAULT_CONTEXT_WINDOW = 200_000;

export function resolveContextWindow(
  usage: UsageSnapshot | undefined,
  catalog: SlimCatalog,
): number {
  return (
    usage?.context.limit ||
    lookupModel(catalog, usage?.context.model)?.contextWindow ||
    DEFAULT_CONTEXT_WINDOW
  );
}

/** Percentage of the window in use, or null when the fill isn't known — no turn
 *  has measured it, or compaction voided the last measurement. Gauges render
 *  null as "unknown"; rendering it as 0% would claim an empty window. */
export function contextPercent(usage: UsageSnapshot | undefined, window: number): number | null {
  if (usage?.context.state !== "measured" || window <= 0) return null;
  return Math.min(100, Math.round((usage.context.tokens / window) * 100));
}
