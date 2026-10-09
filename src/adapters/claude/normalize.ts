// Convert a parsed JSONL transcript into a sequence of synthetic
// RawEvents that the reducer can consume. Claude's JSONL stores
// finalized `assistant` / `user` / `result` records (no stream-event
// deltas), and `reduce` already handles those finalized forms — so this
// normalizer is mostly a filter that drops unrelated record kinds.
//
// The one record it reshapes is a prompt sent while a turn was running: claude
// never logs that as a `user` record, only as a `queued_command` attachment,
// which becomes the `user` event it stands for (mirroring `claude_prompt` in
// crates/fletch-core/src/agent/providers/claude.rs, which pairs turn rows with
// those records).

import { asRecord } from "@/adapters/shared/json";
import { fromRecord } from "@/adapters/shared/record-seq";
import type { RawEvent } from "@/adapters/types";
import { transcriptTextContent } from "./content";

const PASS_THROUGH = new Set(["user", "assistant", "result"]);

/** The `user` event a mid-turn prompt's `queued_command` attachment stands
 *  for, or `rec` itself for any other record. A queued shell command
 *  (`commandMode: "bash"`) is no prompt and stays an attachment, which is
 *  dropped. The record's other fields (flags, sub-agent tag) carry over, so
 *  the reducer treats it as it would a `user` record. */
function queuedPrompt(rec: Record<string, unknown>): Record<string, unknown> {
  if (rec.type !== "attachment") return rec;
  const { attachment, ...rest } = rec;
  const queued = asRecord(attachment);
  const mode = queued.commandMode;
  if (queued.type !== "queued_command" || (mode !== undefined && mode !== "prompt")) return rec;
  return { ...rest, type: "user", message: { role: "user", content: queued.prompt } };
}

export function normalizeTranscript(
  lines: unknown[],
  seqs?: readonly number[],
  sessions?: readonly string[],
): RawEvent[] {
  const out: RawEvent[] = [];
  for (const [i, raw] of lines.entries()) {
    const rec = queuedPrompt(asRecord(raw));
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

    out.push(fromRecord(rec as RawEvent, seqs?.[i], sessions?.[i]));
  }
  return out;
}
