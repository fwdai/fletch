// Token usage from OpenCode's on-disk part blobs.
//
// Each `step-finish` part blob carries that step's usage delta and cost:
//   {"type":"step-finish","tokens":{"input":1532,"output":33,"reasoning":51,
//      "cache":{"read":12864,"write":0}},"cost":0}
// `tokens.input` is FRESH input (cache reads are separate), so summing the
// per-step deltas yields the session total. Cost is reported natively (0 for
// local models). OpenCode does not persist a context-window size.

import type { RawEvent, TurnUsage } from "../types";
import { asNumber, asRecord } from "../shared/json";

export function extractUsage(body: RawEvent): TurnUsage | undefined {
  if (body.type !== "step-finish") return undefined;
  const tokens = asRecord(body.tokens);
  const cache = asRecord(tokens.cache);

  const inputTokens = asNumber(tokens.input);
  const outputTokens = asNumber(tokens.output) + asNumber(tokens.reasoning);
  const cacheReadTokens = asNumber(cache.read);
  const cacheWriteTokens = asNumber(cache.write);
  if (inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens === 0) {
    return undefined;
  }

  return {
    inputTokens,
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens,
    costUsd: asNumber(body.cost),
    context: { input: inputTokens, cacheRead: cacheReadTokens, cacheWrite: cacheWriteTokens },
  };
}
