// Token usage from OpenCode records.
//
// OpenCode reports usage in three shapes depending on the source:
//   - LIVE `run --format json` stream — the per-step delta nested under `.part`:
//       {"type":"step_finish","part":{"tokens":{…},"cost":0,"modelID":"…"}}
//     This is the only path that fires for Quorum: `opencode run` never writes
//     the on-disk blob store, so usage is captured live (persistLiveUsage) and
//     stored into session_records.
//   - ON-DISK assistant message blob (when a transcript is read):
//       {"role":"assistant","modelID":"…","tokens":{…},"cost":0}
//   - a bare step-finish part (older shape / tests): {"type":"step-finish","tokens":{…}}
//
// `tokens.input` is FRESH input (cache reads are separate), so summing the
// per-step deltas yields the session total. Cost is reported natively (0 for
// local models). OpenCode does not persist a context-window size — the meter
// resolves that from the catalog via `model`.

import { asNumber, asRecord } from "../shared/json";
import type { RawEvent, TurnUsage } from "../types";

export function extractUsage(body: RawEvent): TurnUsage | undefined {
  const isLiveFinish = body.type === "step_finish";
  const carriesUsage = isLiveFinish || body.type === "step-finish" || body.role === "assistant";
  if (!carriesUsage) return undefined;
  // The live event nests the usage under `.part`; the other shapes carry it
  // directly on the record body.
  const src = isLiveFinish ? asRecord(body.part) : body;
  const tokens = asRecord(src.tokens);
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
    costUsd: asNumber(src.cost),
    context: { input: inputTokens, cacheRead: cacheReadTokens, cacheWrite: cacheWriteTokens },
    ...(typeof src.modelID === "string" ? { model: src.modelID } : {}),
  };
}
