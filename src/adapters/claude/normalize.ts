// Convert a parsed JSONL transcript into a sequence of synthetic
// RawEvents that the reducer can consume. Claude's JSONL stores
// finalized `assistant` / `user` / `result` records (no stream-event
// deltas), and `reduce` already handles those finalized forms — so this
// normalizer is essentially a filter that drops unrelated record kinds.

import type { RawEvent } from "../types";
import { asRecord } from "../shared/json";
import { transcriptTextContent } from "./content";

const PASS_THROUGH = new Set(["user", "assistant", "result"]);

export function normalizeTranscript(lines: unknown[]): RawEvent[] {
  const out: RawEvent[] = [];
  for (const raw of lines) {
    const rec = asRecord(raw);
    const type = typeof rec.type === "string" ? rec.type : undefined;
    if (!type || !PASS_THROUGH.has(type)) continue;

    if (type === "user" || type === "assistant") {
      // Empty-content turns occur in claude's JSONL (tool-only turns,
      // resumed sessions). Skip them; the reducer would otherwise
      // produce no-ops anyway, but skipping keeps the synthetic stream
      // smaller and avoids spurious dedup decisions.
      const message = asRecord(rec.message);
      const text = transcriptTextContent(message.content);
      const hasText = text.length > 0;
      const hasBlocks =
        Array.isArray(message.content) &&
        (message.content as unknown[]).some((b) => {
          const block = asRecord(b);
          return block.type === "tool_use" || block.type === "tool_result";
        });
      if (!hasText && !hasBlocks) continue;
    }

    out.push(rec as RawEvent);
  }
  return out;
}
