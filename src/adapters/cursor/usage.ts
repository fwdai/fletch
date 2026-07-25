// Token usage from Cursor's `result` event.
//
// cursor-agent never persists usage to its on-disk transcript — it emits it
// once per turn on the live `result` event (Claude-shaped, but camelCase):
//   {"type":"result","subtype":"success",…,"request_id":"…","usage":{
//      "inputTokens":2,"outputTokens":122,
//      "cacheReadTokens":0,"cacheWriteTokens":27987}}
// The adapter sets `persistLiveUsage`, so the store writes this event into
// session_records (`source = 'live_compiled'`, keyed by `request_id`) at
// turn-end and it aggregates from records like every other agent.
//
// That is also why the adapter declares `partial` coverage: a turn that ran
// while Fletch wasn't listening left nothing on disk to read later, so cursor
// totals are a floor rather than a total, and the UI says so instead of
// presenting the gap as a complete figure. `inputTokens` is fresh input
// (excludes cache), Anthropic-style.

import { asNumber, asRecord } from "@/adapters/shared/json";
import { requestEvent } from "@/adapters/shared/usage";
import type { RawEvent } from "@/adapters/types";
import type { UsageEvent } from "@/adapters/usage";

export function usageEvents(body: RawEvent): UsageEvent[] {
  if (body.type !== "result") return [];
  const usage = asRecord(body.usage);
  return requestEvent({
    input: asNumber(usage.inputTokens),
    output: asNumber(usage.outputTokens),
    cacheRead: asNumber(usage.cacheReadTokens),
    cacheWrite: asNumber(usage.cacheWriteTokens),
    id: typeof body.request_id === "string" ? body.request_id : undefined,
  });
}
