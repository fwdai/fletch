// Cursor transcript replay.
//
// cursor-agent persists `~/.cursor/projects/<slug>/agent-transcripts/<id>/<id>.jsonl`.
// Each line is Claude-shaped content (text / tool_use blocks) but with `role`
// at the top level instead of `type`, so cursor/reduce.ts (which delegates
// user/assistant to the Claude reducer) handles it once we rename role→type.
//
// Two on-disk quirks: tool_use blocks carry NO `id` (Claude's do), and there
// are NO tool_result rows (tool outputs aren't persisted). We synthesize a
// stable id per tool_use so multiple calls don't collapse in upsertToolCall;
// tool calls therefore render without results, which is expected for Cursor.

import type { RawEvent } from "../types";
import { asBlockList, asRecord } from "../shared/json";

export function normalizeTranscript(lines: unknown[]): RawEvent[] {
  const out: RawEvent[] = [];
  let toolSeq = 0;
  for (const line of lines) {
    const rec = asRecord(line);
    const role = rec.role;
    if (role !== "user" && role !== "assistant") continue; // drop unknown roles
    const msg = asRecord(rec.message);
    const content = asBlockList(msg.content).map((b) =>
      b.type === "tool_use" && b.id == null
        ? { ...b, id: `cursor-tool-${toolSeq++}` }
        : b,
    );
    out.push({ type: role, message: { ...msg, content } });
  }
  return out;
}
