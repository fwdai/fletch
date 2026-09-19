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
//
// A sub-agent (`Task` tool) writes its own file, `<id>/subagents/<uuid>.jsonl`,
// in the same record shape. The sync ingests it into session_records with a
// top-level `parent_tool_use_id` (the live-wire tag the Claude reducer nests
// by), carried through here. That id is derived from the Task's prompt — the
// only thing the two files share — and `cursorTaskId` gives the Task block the
// same one, so the sub-agent's turns thread under it on replay.

import { asBlockList, asRecord } from "@/adapters/shared/json";
import type { RawEvent } from "@/adapters/types";

/** The replay id of a Cursor Task call, from its prompt. FNV-1a (32-bit) over
 *  the prompt's UTF-16 code units, `cursor-task-<8 hex>` — computed identically
 *  by the sync (`cursor_task_id` in crates/fletch-core/src/agent/providers/
 *  cursor.rs), which tags the sub-agent's records with it. Keep the two in
 *  step: a drift here silently un-nests every replayed Cursor sub-agent. */
export function cursorTaskId(prompt: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < prompt.length; i += 1) {
    h = Math.imul(h ^ prompt.charCodeAt(i), 0x01000193);
  }
  return `cursor-task-${(h >>> 0).toString(16).padStart(8, "0")}`;
}

export function normalizeTranscript(lines: unknown[]): RawEvent[] {
  const out: RawEvent[] = [];
  let toolSeq = 0;
  for (const line of lines) {
    const rec = asRecord(line);
    const role = rec.role;
    if (role !== "user" && role !== "assistant") continue; // drop unknown roles
    const msg = asRecord(rec.message);
    const content = asBlockList(msg.content).map((b) => {
      if (b.type !== "tool_use" || b.id != null) return b;
      const prompt = b.name === "Task" ? asRecord(b.input).prompt : undefined;
      const id = typeof prompt === "string" ? cursorTaskId(prompt) : `cursor-tool-${toolSeq++}`;
      return { ...b, id };
    });
    const ev: RawEvent = { type: role, message: { ...msg, content } };
    if (typeof rec.parent_tool_use_id === "string") ev.parent_tool_use_id = rec.parent_tool_use_id;
    out.push(ev);
  }
  return out;
}
