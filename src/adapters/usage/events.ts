// The vocabulary adapters use to report token usage.
//
// A transcript is not "one line, one API call", which is why summing lines gets
// the wrong answer. Adapters therefore say what a record MEANS, in three kinds
// of event, and `index.ts` owns how each kind accumulates:
//
//   request  — one billed call happened, with these counts (claude, cursor,
//              opencode, pi).
//   counter  — the provider's running total now reads this (codex).
//   boundary — the context window was discarded and rebuilt (claude compaction).

/** Token counts for one call. Two invariants every adapter normalizes to,
 *  because the vendors disagree: `input` is FRESH input — cache reads and
 *  writes are never folded into it — and `output` INCLUDES reasoning tokens.
 *  Codex bills reasoning inside `output_tokens` while OpenCode reports it
 *  alongside a reduced `output`; normalizing here is what keeps that from
 *  becoming a per-provider accumulation rule. */
export interface TokenCounts {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

export const NO_TOKENS: TokenCounts = Object.freeze({
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheWrite: 0,
});

export function totalTokens(t: TokenCounts): number {
  return t.input + t.output + t.cacheRead + t.cacheWrite;
}

/** What occupies a context window, split by cache state. The semantic split the
 *  design mocks up (system / conversation / reasoning) isn't recoverable from
 *  any agent's transcript; this is the truthful equivalent. */
export interface WindowFill {
  input: number;
  cacheRead: number;
  cacheWrite: number;
}

export type UsageEvent =
  | {
      kind: "request";
      /** The call's identity in the provider's terms. This is what makes spend
       *  a set rather than a running sum: Claude appends a line every time it
       *  re-writes a streaming response — same id, growing output — and those
       *  are one billed call, not several. Omit when the record carries no
       *  identity; it then stands alone, which is the safe reading. */
      id?: string;
      model?: string;
      tokens: TokenCounts;
      /** Dollar cost, only when the provider prices the call (opencode, pi). */
      costUsd?: number;
      /** This call ran in its OWN context window — a Claude subagent, logged
       *  into the parent transcript. Real spend, but it says nothing about how
       *  full the main conversation is, so it must not move the gauge. */
      ownWindow?: boolean;
    }
  | {
      kind: "counter";
      model?: string;
      totals: TokenCounts;
      /** The window as of this counter, when the provider states it. */
      window?: WindowFill;
      /** Window size in tokens, when the provider states it (codex). */
      limit?: number;
    }
  | {
      kind: "boundary";
      /** Occupancy right after the boundary, when the provider reports it
       *  (Claude's `compactMetadata.postTokens`). Absent = unknown until the
       *  next turn measures it, which the snapshot says rather than guessing. */
      window?: WindowFill;
    };

/** Whether records can be trusted to hold every call the session made.
 *  `partial` = usage exists only on the live stream (cursor, opencode), so a
 *  turn that ran while Fletch wasn't listening left nothing to re-read. */
export type Coverage = "complete" | "partial";
