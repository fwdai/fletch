// Pi transcript replay.
//
// Pi persists `~/.pi/agent/sessions/<slug>/<ts>_<id>.jsonl`. Unlike its live
// `--mode json` stream (message_start/_update/_end, tool_execution_*, agent_end),
// the on-disk lines are settled `type:"message"` records with a full message
// object — role `user` / `assistant` / `toolResult` — preceded by
// session/model_change/thinking_level_change preamble lines.
//
// The reducer (reduce.ts) was built on the live stream: it folds whole messages
// off `message_end` and renders tool results off `tool_execution_end` (skipping
// toolResult-role messages). So we translate the on-disk shape into those two
// events and drop everything else.

import { asRecord } from "@/adapters/shared/json";
import type { RawEvent } from "@/adapters/types";

export function normalizeTranscript(lines: unknown[]): RawEvent[] {
  const out: RawEvent[] = [];
  for (const line of lines) {
    const rec = asRecord(line);
    if (rec.type !== "message") continue; // drop session/model_change/thinking_level_change/unknown
    const msg = asRecord(rec.message);

    if (msg.role === "toolResult") {
      // reduce renders results off tool_execution_end, not toolResult messages.
      out.push({
        type: "tool_execution_end",
        toolCallId: msg.toolCallId,
        result: { content: msg.content },
        isError: msg.isError === true,
      });
    } else {
      // user / assistant: reduce's message_end arm parses the content blocks
      // (text / thinking / toolCall).
      out.push({ type: "message_end", message: msg });
    }
  }
  return out;
}
