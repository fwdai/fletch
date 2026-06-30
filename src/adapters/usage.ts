// Fold per-record token usage into a per-agent cumulative total.
//
// Usage is computed from session_records — the canonical, persisted transcript
// store — NOT the ephemeral live stream, so totals survive restarts and a turn
// rendered both live and from records is never counted twice. Each adapter's
// `extractUsage` reads its agent's on-disk body shape (see <agent>/usage.ts);
// this fold is provider-agnostic.

import type { SessionRecord } from "@/api";
import { getAdapter } from "./index";
import type { TurnUsage } from "./types";

export interface AgentUsage {
  /** Cumulative fresh (non-cached) input tokens across the session. */
  inputTokens: number;
  /** Cumulative output tokens (incl. reasoning) across the session. */
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  /** Cumulative dollar cost; 0 when no agent in the session reports cost. */
  costUsd: number;
  // ── latest turn's context-window composition (drives the meter + its bar) ──
  /** Fresh, non-cached input in the current window. */
  contextInput: number;
  /** Reused/cached context in the current window. */
  contextCacheRead: number;
  /** Newly-cached tokens in the current window (0 for codex). */
  contextCacheWrite: number;
  /** Context-window fill in tokens = the three context* parts above. */
  contextTokens: number;
  /** Model context window in tokens; 0 when the agent doesn't report one (the
   *  UI falls back to a default). */
  contextWindow: number;
  model?: string;
}

export const EMPTY_USAGE: AgentUsage = Object.freeze({
  inputTokens: 0,
  outputTokens: 0,
  cacheReadTokens: 0,
  cacheWriteTokens: 0,
  costUsd: 0,
  contextInput: 0,
  contextCacheRead: 0,
  contextCacheWrite: 0,
  contextTokens: 0,
  contextWindow: 0,
});

/** True once any token field is non-zero — i.e. the agent reported usage. */
export function hasUsage(u: AgentUsage): boolean {
  return u.inputTokens + u.outputTokens + u.cacheReadTokens + u.cacheWriteTokens > 0;
}

/** Apply one extracted `TurnUsage` to a running total, returning a new object.
 *  Shared by the records fold and the live accumulator (cursor) so both treat
 *  cumulative-vs-delta and the context breakdown identically. */
export function addTurnUsage(acc: AgentUsage, u: TurnUsage): AgentUsage {
  const next: AgentUsage = { ...acc };
  if (u.cumulative) {
    // Running total — the latest record wins, don't sum.
    next.inputTokens = u.inputTokens;
    next.outputTokens = u.outputTokens;
    next.cacheReadTokens = u.cacheReadTokens;
    next.cacheWriteTokens = u.cacheWriteTokens;
    if (u.costUsd != null) next.costUsd = u.costUsd;
  } else {
    next.inputTokens += u.inputTokens;
    next.outputTokens += u.outputTokens;
    next.cacheReadTokens += u.cacheReadTokens;
    next.cacheWriteTokens += u.cacheWriteTokens;
    if (u.costUsd != null) next.costUsd += u.costUsd;
  }
  if (u.context) {
    const fill = u.context.input + u.context.cacheRead + u.context.cacheWrite;
    if (fill > 0) {
      // Latest turn wins — the window reflects the most recent turn, not a sum.
      next.contextInput = u.context.input;
      next.contextCacheRead = u.context.cacheRead;
      next.contextCacheWrite = u.context.cacheWrite;
      next.contextTokens = fill;
    }
  }
  if (u.contextWindow != null && u.contextWindow > 0) next.contextWindow = u.contextWindow;
  if (u.model) next.model = u.model;
  return next;
}

/** Fold a session's records into one cumulative usage total. Returns the shared
 *  `EMPTY_USAGE` when the provider doesn't extract usage (antigravity) or no
 *  record carried any. Cursor is included: its live `result` is persisted into
 *  session_records (see `persistLiveUsage`), so it folds here like the rest.
 *  Defensive: a throwing extractor skips its record, not the whole fold. */
export function usageFromRecords(
  provider: string | undefined,
  records: SessionRecord[],
): AgentUsage {
  const adapter = getAdapter(provider);
  if (!adapter.extractUsage) return EMPTY_USAGE;

  let acc: AgentUsage = { ...EMPTY_USAGE };
  for (const rec of records) {
    let u: TurnUsage | undefined;
    try {
      u = adapter.extractUsage(rec.body);
    } catch {
      u = undefined;
    }
    if (u) acc = addTurnUsage(acc, u);
  }
  return hasUsage(acc) ? acc : EMPTY_USAGE;
}
