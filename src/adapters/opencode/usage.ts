// Token usage from OpenCode records.
//
// Usage is folded over session_records, which hold OpenCode's ON-DISK shape:
// each assistant message blob carries that step's usage delta, cost, and model:
//   {"role":"assistant","modelID":"…","tokens":{"input":98,"output":18,
//      "reasoning":0,"cache":{"read":10624,"write":0}},"cost":0}
// The live `run --format json` stream instead emits the same fields on a
// `step-finish` event; we accept either so the extractor works whether fed the
// on-disk record (the usual path) or a live event.
//
// `tokens.input` is FRESH input (cache reads are separate), so summing the
// per-step deltas yields the session total. Cost is reported natively (0 for
// local models). OpenCode does not persist a context-window size — the meter
// resolves that from the catalog via `model`.

import type { RawEvent, TurnUsage } from "../types";
import { asNumber, asRecord } from "../shared/json";

export function extractUsage(body: RawEvent): TurnUsage | undefined {
  // On-disk assistant message (the folded shape) or a live step-finish event.
  const carriesUsage = body.role === "assistant" || body.type === "step-finish";
  if (!carriesUsage) return undefined;
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
    ...(typeof body.modelID === "string" ? { model: body.modelID } : {}),
  };
}
