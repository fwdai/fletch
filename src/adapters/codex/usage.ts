// Token usage from Codex's on-disk rollout.
//
// Codex doesn't log individual calls; it logs a counter. Each `token_count`
// event (on the `event_msg` channel) restates the session's running total plus
// the last turn's contribution:
//   {"type":"event_msg","payload":{"type":"token_count","info":{
//      "total_token_usage":{"input_tokens","cached_input_tokens",
//                           "cache_write_input_tokens","output_tokens",
//                           "reasoning_output_tokens","total_tokens"},
//      "last_token_usage":{…same shape…},
//      "model_context_window":258400}}}
//
// The counter carries no model; the rollout states that separately on
// `turn_context` records (`payload.model`), which precede the turn they
// configure. Those become `model` hint events so each turn's delta can be
// attributed — and priced — against the model actually in force.
//
// So the record becomes a `counter` event and the aggregator differences it —
// codex re-emits identical snapshots (summing would double-count) and restarts
// the counter when a thread continues in a fresh rollout (latest-wins would
// erase everything before the restart).
//
// Two normalizations the vendor's field names hide. `input_tokens` INCLUDES
// cached input, so fresh input is the difference. And `output_tokens` ALREADY
// INCLUDES `reasoning_output_tokens` — codex's own `blended_total()` adds only
// non-cached input and output — so reasoning is reported as the subset it is
// rather than added on top.
//
// Window occupancy is `last_token_usage.total_tokens`, the field codex itself
// calls `tokens_in_context_window`. It counts the turn's output as well as its
// input, because that output is part of the next request's prompt; the input
// side alone undercounts a reasoning-heavy turn by its whole response. Codex's
// TUI additionally discounts a fixed 12k baseline for the system prompt and
// tools when it prints a percentage, so its "% left" reads slightly emptier
// than the raw occupancy shown here.

import { asNumber, asRecord } from "@/adapters/shared/json";
import type { RawEvent } from "@/adapters/types";
import type { TokenCounts, UsageEvent, WindowFill } from "@/adapters/usage";

export function usageEvents(body: RawEvent): UsageEvent[] {
  // The counter itself never names a model; `turn_context` does, and it always
  // precedes the turn it configures. Emitting it as a standalone `model` hint
  // lets the aggregator attribute each turn's delta to the model in force —
  // the only way a codex session can be priced — without inventing a fake
  // counter reading that the differencing would then have to special-case.
  if (body.type === "turn_context") {
    const model = asRecord(body.payload).model;
    return typeof model === "string" && model ? [{ kind: "model", model }] : [];
  }

  if (body.type !== "event_msg") return [];
  const payload = asRecord(body.payload);
  if (payload.type !== "token_count") return [];

  const info = asRecord(payload.info);
  const totals = tokenCounts(asRecord(info.total_token_usage));
  if (totals.input + totals.output === 0) return [];

  const limit = asNumber(info.model_context_window);
  const window = windowFill(asRecord(info.last_token_usage));
  return [
    {
      kind: "counter",
      totals,
      ...(window ? { window } : {}),
      ...(limit > 0 ? { limit } : {}),
    },
  ];
}

function tokenCounts(usage: Record<string, unknown>): TokenCounts {
  const cacheRead = asNumber(usage.cached_input_tokens);
  return {
    input: Math.max(0, asNumber(usage.input_tokens) - cacheRead),
    output: asNumber(usage.output_tokens),
    cacheRead,
    cacheWrite: asNumber(usage.cache_write_input_tokens),
  };
}

/** Split the last turn's window occupancy into the meter's cache-state slices.
 *  Older rollouts omit `total_tokens`, so fall back to the input side alone.
 *  Cached and newly-cached tokens are carved out of that occupancy; whatever
 *  remains — fresh prompt plus the turn's own output — is uncached input. */
function windowFill(last: Record<string, unknown>): WindowFill | undefined {
  const fill = asNumber(last.total_tokens) || asNumber(last.input_tokens);
  if (fill <= 0) return undefined;
  const cacheRead = Math.min(asNumber(last.cached_input_tokens), fill);
  const cacheWrite = Math.min(asNumber(last.cache_write_input_tokens), fill - cacheRead);
  return { input: fill - cacheRead - cacheWrite, cacheRead, cacheWrite };
}
