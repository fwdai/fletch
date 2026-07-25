// Token usage from Pi's on-disk transcript.
//
// Pi persists settled `type:"message"` records; an assistant message carries
// its call's usage with a native cost:
//   {"type":"message","message":{"id":"…","role":"assistant","model":"claude-…",
//      "usage":{"input":2,"output":14,"cacheRead":0,"cacheWrite":3159,
//               "totalTokens":3175,"cost":{"total":0.0201}}}}
// `input` is fresh input (cache read/write are separate) and `totalTokens` is
// the sum of the four, so the fields map straight across. Pi persists no
// context-window size — the meter resolves that from the catalog via `model`.

import { asNumber, asRecord } from "@/adapters/shared/json";
import { requestEvent } from "@/adapters/shared/usage";
import type { RawEvent } from "@/adapters/types";
import type { UsageEvent } from "@/adapters/usage";

export function usageEvents(body: RawEvent): UsageEvent[] {
  if (body.type !== "message") return [];
  const message = asRecord(body.message);
  if (message.role !== "assistant") return [];

  const usage = asRecord(message.usage);
  return requestEvent({
    input: asNumber(usage.input),
    output: asNumber(usage.output),
    cacheRead: asNumber(usage.cacheRead),
    cacheWrite: asNumber(usage.cacheWrite),
    costUsd: asNumber(asRecord(usage.cost).total),
    model: typeof message.model === "string" ? message.model : undefined,
    id: typeof message.id === "string" ? message.id : undefined,
  });
}
