// Token usage from Claude's on-disk transcript.
//
// Each persisted `assistant` record carries a per-message usage delta on
// `message.usage`:
//   {"type":"assistant","message":{"model":"claude-…","usage":{
//      "input_tokens":2,"output_tokens":300,
//      "cache_creation_input_tokens":10783,"cache_read_input_tokens":7900}}}
// `input_tokens` is the FRESH (non-cached) input — Anthropic excludes cache
// reads/writes from it — so context fill is the sum of all three input fields.
// Claude does not report a context-window size or cost on disk.

import { asNumber, asRecord } from "../shared/json";
import type { RawEvent, TurnUsage } from "../types";

export function extractUsage(body: RawEvent): TurnUsage | undefined {
  if (body.type !== "assistant") return undefined;
  const message = asRecord(body.message);
  const usage = asRecord(message.usage);
  const inputTokens = asNumber(usage.input_tokens);
  const outputTokens = asNumber(usage.output_tokens);
  const cacheReadTokens = asNumber(usage.cache_read_input_tokens);
  const cacheWriteTokens = asNumber(usage.cache_creation_input_tokens);
  if (inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens === 0) {
    return undefined;
  }
  return {
    inputTokens,
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens,
    context: { input: inputTokens, cacheRead: cacheReadTokens, cacheWrite: cacheWriteTokens },
    model: typeof message.model === "string" ? message.model : undefined,
  };
}
