// Token usage: from persisted session records to one snapshot per agent.
//
//   session_records ──adapter.usageEvents(body)──▶ UsageEvent[] ──▶ UsageSnapshot
//
// Records are the canonical, persisted transcript store — not the ephemeral
// live stream — so totals survive restarts and a turn rendered both live and
// from records is never counted twice.
//
// The snapshot holds two numbers that behave differently, which is the whole
// point of keeping them apart:
//
//   SPEND accumulates. Every call the session made, summed — that total is what
//   the session cost, and it only grows. It is accumulated as a SET keyed by
//   call identity rather than a running sum, so a record that restates a call
//   (Claude re-writes a response line per streaming chunk) is counted once
//   while genuinely separate calls all still add up.
//
//   CONTEXT does not accumulate. It is a measurement of the live window taken
//   at the last turn: subagents have their own, and compaction throws it away.
//   Summing it, or reading it off whichever record happened to come last, both
//   answer the wrong question.

import type { SessionRecord } from "@/api";
import { getAdapter } from "../index";
import {
  type Coverage,
  NO_TOKENS,
  type TokenCounts,
  totalTokens,
  type UsageEvent,
  type WindowFill,
} from "./events";

export type { Coverage, TokenCounts, UsageEvent, WindowFill } from "./events";
export { NO_TOKENS, totalTokens } from "./events";
export { contextPercent, DEFAULT_CONTEXT_WINDOW, resolveContextWindow } from "./window";

/** Whether the context figure means anything right now. `reset` = compaction
 *  voided the last measurement and no turn has taken a new one, so the honest
 *  answer is "unknown" — never 0%. */
export type ContextState = "measured" | "reset" | "unknown";

export interface UsageSnapshot {
  /** Session TOTAL — every call summed. Grows for the life of the session. */
  spend: {
    tokens: TokenCounts;
    /** Null when no agent in the session prices its own calls (claude, codex
     *  and cursor report tokens but no dollars); 0 is a real free session. */
    costUsd: number | null;
  };
  /** The LIVE context window, as of the last turn that measured it. */
  context: {
    state: ContextState;
    fill: WindowFill;
    tokens: number;
    /** Provider-stated window size, or 0 — the caller then resolves it from the
     *  model catalog (see `resolveContextWindow`). */
    limit: number;
    /** The model whose window this describes; never a subagent's. */
    model?: string;
  };
  coverage: Coverage;
}

const EMPTY_FILL: WindowFill = Object.freeze({ input: 0, cacheRead: 0, cacheWrite: 0 });

export const EMPTY_SNAPSHOT: UsageSnapshot = Object.freeze({
  spend: Object.freeze({ tokens: NO_TOKENS, costUsd: null }),
  context: Object.freeze({
    state: "unknown" as ContextState,
    fill: EMPTY_FILL,
    tokens: 0,
    limit: 0,
  }),
  coverage: "complete" as Coverage,
});

/** True once the session has spent anything. Distinguishes "nothing yet" from
 *  "an agent that reports no usage", which is why the store guards writes with
 *  it rather than overwriting a good snapshot with an empty one. */
export function hasUsage(u: UsageSnapshot): boolean {
  return totalTokens(u.spend.tokens) > 0;
}

/** Aggregate one session's records into a usage snapshot. Returns the shared
 *  `EMPTY_SNAPSHOT` when the provider reports no usage (antigravity) or no
 *  record carried any. Cursor is included: its live `result` is persisted into
 *  session_records (see `persistLiveUsage`), so it aggregates like the rest.
 *
 *  Defensive: a record we can't read costs one record, not the session. */
export function usageFromRecords(
  provider: string | undefined,
  records: SessionRecord[],
): UsageSnapshot {
  const adapter = getAdapter(provider);
  if (!adapter.usageEvents) return EMPTY_SNAPSHOT;

  const events: UsageEvent[] = [];
  for (const rec of records) {
    try {
      events.push(...adapter.usageEvents(rec.body));
    } catch {
      // Unreadable record — skip it, keep the rest of the session.
    }
  }

  const snapshot = aggregate(events, adapter.usageCoverage ?? "complete");
  return hasUsage(snapshot) ? snapshot : EMPTY_SNAPSHOT;
}

/** Fold events into a snapshot. Exported for tests; production callers go
 *  through `usageFromRecords`. */
export function aggregate(events: UsageEvent[], coverage: Coverage = "complete"): UsageSnapshot {
  // Spend, keyed by call identity. Records that restate one call collapse onto
  // the entry already there, keeping whichever is more complete — Claude's
  // early streaming snapshots carry a partial output count and the settled one
  // carries the final count. Records with no identity get a unique key, so they
  // are never mistaken for duplicates of each other.
  const calls = new Map<string, { tokens: TokenCounts; costUsd?: number }>();
  let unkeyed = 0;
  // Keys for entries with no provider identity. The prefix can't collide with a
  // real id, and the counter can't collide with itself.
  const ownKey = () => `\u0000${unkeyed++}`;

  // Codex reports a running total instead of individual calls. Neither summing
  // (it re-emits identical snapshots) nor "latest wins" (it restarts at zero in
  // a resumed rollout, or when a fork inherits its parent's records) is right,
  // so consecutive snapshots are differenced, and a snapshot that went
  // backwards is treated as a restart whose whole value is new spend.
  let counter: TokenCounts | undefined;

  let state: ContextState = "unknown";
  let fill: WindowFill = EMPTY_FILL;
  let limit = 0;
  let model: string | undefined;
  let priced = false;

  const observe = (next: WindowFill, nextModel?: string) => {
    if (next.input + next.cacheRead + next.cacheWrite <= 0) return;
    state = "measured";
    fill = next;
    if (nextModel) model = nextModel;
  };

  for (const event of events) {
    if (event.kind === "boundary") {
      // The conversation was thrown away; the fill we were reporting describes
      // something that no longer exists. Spend is untouched — it was spent.
      state = "reset";
      fill = EMPTY_FILL;
      if (event.window) observe(event.window);
      continue;
    }

    if (event.kind === "counter") {
      const delta = counterDelta(event.totals, counter);
      counter = event.totals;
      if (totalTokens(delta) > 0) calls.set(ownKey(), { tokens: delta });
      if (event.limit && event.limit > 0) limit = event.limit;
      if (event.window) observe(event.window, event.model);
      continue;
    }

    if (totalTokens(event.tokens) === 0 && event.costUsd == null) continue;
    const key = event.id ?? ownKey();
    const seen = calls.get(key);
    if (!seen || totalTokens(event.tokens) > totalTokens(seen.tokens)) {
      calls.set(key, {
        tokens: event.tokens,
        ...(event.costUsd != null ? { costUsd: event.costUsd } : {}),
      });
    }
    if (event.costUsd != null) priced = true;
    // A call's own input side IS what occupied the window when it was made —
    // for every Anthropic-shaped transcript. A subagent's does not, so it is
    // spend only.
    if (!event.ownWindow) {
      observe(
        {
          input: event.tokens.input,
          cacheRead: event.tokens.cacheRead,
          cacheWrite: event.tokens.cacheWrite,
        },
        event.model,
      );
    }
  }

  let tokens: TokenCounts = NO_TOKENS;
  let costUsd = 0;
  for (const call of calls.values()) {
    tokens = {
      input: tokens.input + call.tokens.input,
      output: tokens.output + call.tokens.output,
      cacheRead: tokens.cacheRead + call.tokens.cacheRead,
      cacheWrite: tokens.cacheWrite + call.tokens.cacheWrite,
    };
    costUsd += call.costUsd ?? 0;
  }

  return {
    spend: { tokens, costUsd: priced ? costUsd : null },
    context: {
      state,
      fill,
      tokens: fill.input + fill.cacheRead + fill.cacheWrite,
      limit,
      ...(model ? { model } : {}),
    },
    coverage,
  };
}

function counterDelta(totals: TokenCounts, previous: TokenCounts | undefined): TokenCounts {
  if (!previous || totalTokens(totals) < totalTokens(previous)) return totals;
  return {
    input: Math.max(0, totals.input - previous.input),
    output: Math.max(0, totals.output - previous.output),
    cacheRead: Math.max(0, totals.cacheRead - previous.cacheRead),
    cacheWrite: Math.max(0, totals.cacheWrite - previous.cacheWrite),
  };
}
