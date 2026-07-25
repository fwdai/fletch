// Token usage from Claude's on-disk transcript.
//
// An `assistant` record carries its call's usage on `message.usage`:
//   {"type":"assistant","requestId":"req_…","isSidechain":false,
//    "message":{"id":"msg_…","model":"claude-…","usage":{
//      "input_tokens":2,"output_tokens":300,
//      "cache_creation_input_tokens":10783,"cache_read_input_tokens":7900}}}
// `input_tokens` is the FRESH input — Anthropic excludes cache reads/writes
// from it — so the window the call was made against is the sum of all three
// input fields. Claude reports neither a context-window size nor a cost.
//
// Three things about this transcript are not "one line, one call", and each
// becomes an explicit part of the event rather than an accumulation rule:
//
//   STREAMING RE-WRITES. Claude appends a new line each time it re-writes a
//   response as it streams — same `message.id` and `requestId`, identical input
//   and cache counts, a growing `output_tokens`. They are one billed call, so
//   they share one request id and the ledger collapses them.
//
//   SUBAGENTS. Task/Agent turns land in the parent transcript tagged
//   `isSidechain: true`. They cost real tokens against their own context
//   window, so they count as spend but must never move the main gauge.
//
//   COMPACTION. `{"type":"system","subtype":"compact_boundary"}` records the
//   moment the conversation was replaced by a summary. Nothing measures the new
//   window until the next turn runs, so without this event the meter keeps
//   reporting the fill the user just spent a turn getting rid of.

import { asNumber, asRecord } from "@/adapters/shared/json";
import { requestEvent } from "@/adapters/shared/usage";
import type { RawEvent } from "@/adapters/types";
import type { UsageEvent } from "@/adapters/usage";

export function usageEvents(body: RawEvent): UsageEvent[] {
  if (body.type === "system") return boundaryEvents(body);
  if (body.type !== "assistant") return [];
  // An API-error envelope replays the failed call's usage under a new line, and
  // the placeholder `<synthetic>` model is the CLI talking to itself (interrupt
  // notices, error text). Neither is a billed call of its own.
  if (body.isApiErrorMessage === true) return [];
  const message = asRecord(body.message);
  if (message.model === "<synthetic>") return [];

  const usage = asRecord(message.usage);
  return requestEvent({
    input: asNumber(usage.input_tokens),
    output: asNumber(usage.output_tokens),
    cacheRead: asNumber(usage.cache_read_input_tokens),
    cacheWrite: cacheCreationTokens(usage),
    id: requestId(body, message),
    model: typeof message.model === "string" ? message.model : undefined,
    ownWindow: body.isSidechain === true,
  });
}

/** Newer Claude builds break cache writes out by TTL under `cache_creation`
 *  while keeping the flat `cache_creation_input_tokens` beside it; the
 *  breakdown is authoritative when present. */
function cacheCreationTokens(usage: Record<string, unknown>): number {
  const breakdown = asRecord(usage.cache_creation);
  const split =
    asNumber(breakdown.ephemeral_5m_input_tokens) + asNumber(breakdown.ephemeral_1h_input_tokens);
  return split > 0 ? split : asNumber(usage.cache_creation_input_tokens);
}

/** Identity of the underlying API call. `message.id` alone would merge a retry
 *  of the same message, so the request id joins it when present. Undefined
 *  without a message id: an unidentifiable record must stand alone rather than
 *  collide with every other unidentifiable one. */
function requestId(body: RawEvent, message: Record<string, unknown>): string | undefined {
  const messageId = typeof message.id === "string" ? message.id : "";
  if (!messageId) return undefined;
  const requestId = typeof body.requestId === "string" ? body.requestId : "";
  return `${messageId}:${requestId}`;
}

/** Compaction markers.
 *
 *  `compact_boundary` means the conversation was discarded for a summary, so
 *  the window we were reporting no longer exists. `compactMetadata.postTokens`
 *  gives the new occupancy when the CLI records it (it is optional); without
 *  it the boundary still fires, and the snapshot reports the window as `reset`
 *  rather than inventing a number.
 *
 *  `microcompact_boundary` only evicts old tool results, so it is honored just
 *  when it states the resulting size — declaring a mostly-full window unknown
 *  would be a worse answer than the one we already have. */
function boundaryEvents(body: RawEvent): UsageEvent[] {
  const micro = body.subtype === "microcompact_boundary";
  if (body.subtype !== "compact_boundary" && !micro) return [];

  const meta = asRecord(micro ? body.microcompactMetadata : body.compactMetadata);
  const postTokens = asNumber(meta.postTokens);
  if (micro && postTokens <= 0) return [];

  return [
    {
      kind: "boundary",
      // The summary is re-sent as fresh input on the next request and cached
      // from there, so it belongs in the `input` slice of the breakdown.
      ...(postTokens > 0 ? { window: { input: postTokens, cacheRead: 0, cacheWrite: 0 } } : {}),
    },
  ];
}
