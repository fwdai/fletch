// Token usage from Pi's on-disk transcript.
//
// Pi persists settled `type:"message"` records; assistant messages carry a
// per-message usage delta with native cost:
//   {"type":"message","message":{"role":"assistant","model":"claude-…",
//      "usage":{"input":2,"output":14,"cacheRead":0,"cacheWrite":3159,
//               "totalTokens":3175,"cost":{"total":0.0201}}}}
// `input` is FRESH input (cache read/write are separate), so summing the
// per-message deltas yields the session total. Pi does not persist a
// context-window size.

import { asNumber, asRecord } from "@/adapters/shared/json";
import { buildTurnUsage } from "@/adapters/shared/usage";
import type { RawEvent, TurnUsage } from "@/adapters/types";

export function extractUsage(body: RawEvent): TurnUsage | undefined {
  if (body.type !== "message") return undefined;
  const message = asRecord(body.message);
  if (message.role !== "assistant") return undefined;

  const usage = asRecord(message.usage);
  return buildTurnUsage({
    inputTokens: asNumber(usage.input),
    outputTokens: asNumber(usage.output),
    cacheReadTokens: asNumber(usage.cacheRead),
    cacheWriteTokens: asNumber(usage.cacheWrite),
    costUsd: asNumber(asRecord(usage.cost).total),
    model: typeof message.model === "string" ? message.model : undefined,
  });
}
