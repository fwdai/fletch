// Token usage from Cursor's `result` event.
//
// Unlike the other agents, cursor-agent does not persist usage to its on-disk
// transcript — it only emits it once per turn on the live `result` event
// (Claude-shaped, but camelCase):
//   {"type":"result","subtype":"success",…,"request_id":"…","usage":{
//      "inputTokens":2,"outputTokens":122,
//      "cacheReadTokens":0,"cacheWriteTokens":27987}}
// The adapter sets `persistLiveUsage`, so the store writes this event into
// session_records (`source = 'live_compiled'`, keyed by `request_id`) at
// turn-end; from then on usage folds from records like every other agent —
// restart-safe. `inputTokens` is fresh (excludes cache), Anthropic-style.

import { asNumber, asRecord } from "../shared/json";
import type { RawEvent, TurnUsage } from "../types";

export function extractUsage(body: RawEvent): TurnUsage | undefined {
  if (body.type !== "result") return undefined;
  const usage = asRecord(body.usage);
  const inputTokens = asNumber(usage.inputTokens);
  const outputTokens = asNumber(usage.outputTokens);
  const cacheReadTokens = asNumber(usage.cacheReadTokens);
  const cacheWriteTokens = asNumber(usage.cacheWriteTokens);
  if (inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens === 0) {
    return undefined;
  }
  return {
    inputTokens,
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens,
    context: { input: inputTokens, cacheRead: cacheReadTokens, cacheWrite: cacheWriteTokens },
  };
}
