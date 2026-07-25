// Token usage from OpenCode records.
//
// OpenCode reports one call's usage in three shapes depending on the source:
//   - LIVE `run --format json` stream — the per-step delta nested under `.part`:
//       {"type":"step_finish","part":{"tokens":{…},"cost":0,"modelID":"…"}}
//     This is the only path that fires for Fletch: `opencode run` never writes
//     the on-disk blob store, so usage is captured live (persistLiveUsage) and
//     stored into session_records.
//   - ON-DISK assistant message blob (when a transcript is read):
//       {"role":"assistant","modelID":"…","tokens":{…},"cost":0}
//   - a bare step-finish part (older shape / tests): {"type":"step-finish","tokens":{…}}
//
// `tokens.input` is already fresh (OpenCode subtracts cache read and write
// itself) and `tokens.output` EXCLUDES `tokens.reasoning`. Cost is priced
// natively (0 for local models). OpenCode persists no context-window size — the
// meter resolves that from the catalog via `model`.
//
// A step is a billed call and a turn can hold several, so steps must not
// collapse into one another: they carry the part's own id when it has one,
// which also makes re-delivering the same step idempotent.

import { asNumber, asRecord } from "@/adapters/shared/json";
import { requestEvent } from "@/adapters/shared/usage";
import type { RawEvent } from "@/adapters/types";
import type { UsageEvent } from "@/adapters/usage";

export function usageEvents(body: RawEvent): UsageEvent[] {
  const isLiveFinish = body.type === "step_finish";
  const carriesUsage = isLiveFinish || body.type === "step-finish" || body.role === "assistant";
  if (!carriesUsage) return [];
  // The live event nests the usage under `.part`; the other shapes carry it
  // directly on the record body.
  const src = isLiveFinish ? asRecord(body.part) : body;
  const tokens = asRecord(src.tokens);
  const cache = asRecord(tokens.cache);

  return requestEvent({
    input: asNumber(tokens.input),
    // OpenCode reports `output` with reasoning already subtracted out, so the
    // two are added back to form the output total the aggregator expects.
    output: asNumber(tokens.output) + asNumber(tokens.reasoning),
    cacheRead: asNumber(cache.read),
    cacheWrite: asNumber(cache.write),
    costUsd: asNumber(src.cost),
    model: typeof src.modelID === "string" ? src.modelID : undefined,
    id: typeof src.id === "string" ? src.id : undefined,
  });
}
