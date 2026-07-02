// Token usage from Codex's on-disk rollout.
//
// Codex emits a `token_count` event (on the `event_msg` channel) carrying both
// a running cumulative total and the last turn's delta:
//   {"type":"event_msg","payload":{"type":"token_count","info":{
//      "total_token_usage":{"input_tokens","cached_input_tokens",
//                           "output_tokens","reasoning_output_tokens"},
//      "last_token_usage":{…same shape…},
//      "model_context_window":258400}}}
// We take the CUMULATIVE `total_token_usage` (latest record wins) rather than
// summing — codex re-emits identical token_count lines, so summing the delta
// would double-count. `input_tokens` INCLUDES cached input, so fresh input is
// `input_tokens - cached_input_tokens`. Context fill is the latest turn's
// `last_token_usage.input_tokens` (the live window size), against
// `model_context_window`.

import { asNumber, asRecord } from "@/adapters/shared/json";
import type { RawEvent, TurnUsage } from "@/adapters/types";

export function extractUsage(body: RawEvent): TurnUsage | undefined {
  if (body.type !== "event_msg") return undefined;
  const payload = asRecord(body.payload);
  if (payload.type !== "token_count") return undefined;

  const info = asRecord(payload.info);
  const total = asRecord(info.total_token_usage);
  const last = asRecord(info.last_token_usage);

  const totalInput = asNumber(total.input_tokens);
  const cacheReadTokens = asNumber(total.cached_input_tokens);
  const outputTokens = asNumber(total.output_tokens) + asNumber(total.reasoning_output_tokens);
  if (totalInput + outputTokens === 0) return undefined;

  // Context fill is the latest turn's window: last_token_usage.input_tokens
  // includes cached, so split it into cached vs fresh. Codex reports no cache
  // write separately.
  const lastInput = asNumber(last.input_tokens);
  const lastCached = asNumber(last.cached_input_tokens);
  const contextWindow = asNumber(info.model_context_window);

  return {
    cumulative: true,
    inputTokens: Math.max(0, totalInput - cacheReadTokens),
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens: 0,
    context:
      lastInput > 0
        ? { input: Math.max(0, lastInput - lastCached), cacheRead: lastCached, cacheWrite: 0 }
        : undefined,
    contextWindow: contextWindow > 0 ? contextWindow : undefined,
  };
}
