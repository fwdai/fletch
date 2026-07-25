// Shared builder for the `request` event four of the five adapters emit.
//
// Its only job is normalization: the zero-token guard in one place, and the
// `TokenCounts` invariants (fresh input, output including reasoning) stated
// where every adapter has to pass through them.

import type { UsageEvent } from "@/adapters/usage";

export interface RequestSpec {
  /** Fresh, non-cached input tokens. */
  input: number;
  /** Output tokens, INCLUDING reasoning. */
  output: number;
  cacheRead: number;
  cacheWrite: number;
  /** Identity of the API call — see `UsageEvent`'s `id`. */
  id?: string;
  model?: string;
  /** Dollar cost, only when the provider prices the call (opencode, pi). */
  costUsd?: number;
  /** The call ran in its own context window (a Claude subagent). */
  ownWindow?: boolean;
}

/** The `request` event for one API call, or nothing when the record carries no
 *  usage — a zero-token record has no usage, it isn't a free call. */
export function requestEvent(spec: RequestSpec): UsageEvent[] {
  const tokens = {
    input: spec.input,
    output: spec.output,
    cacheRead: spec.cacheRead,
    cacheWrite: spec.cacheWrite,
  };
  if (tokens.input + tokens.output + tokens.cacheRead + tokens.cacheWrite === 0) return [];
  return [
    {
      kind: "request",
      tokens,
      ...(spec.id !== undefined ? { id: spec.id } : {}),
      ...(spec.model !== undefined ? { model: spec.model } : {}),
      ...(spec.costUsd !== undefined ? { costUsd: spec.costUsd } : {}),
      ...(spec.ownWindow ? { ownWindow: true } : {}),
    },
  ];
}
