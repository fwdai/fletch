import {
  contextPercent,
  EMPTY_SNAPSHOT,
  resolveContextWindow,
  type UsageSnapshot,
} from "@/adapters/usage";
import type { SlimCatalog } from "@/data/modelCatalog/types";

/** Where a compaction stands: none in sight, one running, or one that is the
 *  last thing to have touched the window (until the next turn measures it). */
export type CompactPhase = "idle" | "compacting" | "done";

/** The share of the window at which the badge turns red. The only alarm: below
 *  it everything stays the brand accent — there is no amber tier. */
export const DANGER_PCT = 90;

export interface WindowSegment {
  key: string;
  label: string;
  tokens: number;
  color: string;
}

/** The live window as the badge draws it. */
export interface ContextView {
  used: number;
  max: number;
  /** Null when the fill isn't known (see `contextPercent`). */
  pct: number | null;
  free: number;
  segments: WindowSegment[];
  tone: "ok" | "danger";
}

/** What the badge shows for an agent with no usage in the store yet: nothing
 *  spent, the window unmeasured, and sized by the model it was spawned with
 *  rather than the default (a 1M-window model would otherwise read as 200k
 *  until its first call). */
export function unspent(model: string | null | undefined): UsageSnapshot {
  if (!model) return EMPTY_SNAPSHOT;
  return { ...EMPTY_SNAPSHOT, context: { ...EMPTY_SNAPSHOT.context, model } };
}

/** The window split by cache state — the truthful stand-in for the design's
 *  system / conversation / reasoning split, which no agent's transcript
 *  exposes. Right after a compaction the fresh input IS the summary the
 *  conversation continues from, so it is named that. */
export function contextView(
  usage: UsageSnapshot,
  catalog: SlimCatalog,
  phase: CompactPhase,
): ContextView {
  const max = resolveContextWindow(usage, catalog);
  const pct = contextPercent(usage, max);
  const { fill, tokens: used } = usage.context;
  const segments: WindowSegment[] = [
    { key: "cacheRead", label: "Cache read", tokens: fill.cacheRead, color: "var(--accent)" },
    { key: "cacheWrite", label: "Cache write", tokens: fill.cacheWrite, color: "var(--info)" },
    {
      key: "input",
      label: phase === "done" ? "Summary" : "Input",
      tokens: fill.input,
      color: "var(--fg-2)",
    },
  ];
  return {
    used,
    max,
    pct,
    free: Math.max(0, max - used),
    segments: segments.filter((s) => s.tokens > 0),
    tone: pct != null && pct >= DANGER_PCT ? "danger" : "ok",
  };
}
