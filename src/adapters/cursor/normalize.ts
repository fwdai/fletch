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
// only thing the two files share — and this module gives the Task block the
// same one, so the sub-agent's turns thread under it on replay. Two Tasks with
// the same prompt would share a hash, so the id also carries the call's
// occurrence number (see `cursorTaskIdNth`).

import { asBlockList, asRecord } from "@/adapters/shared/json";
import type { RawEvent } from "@/adapters/types";

/** The base replay id of a Cursor Task call, from its prompt. FNV-1a (32-bit)
 *  over the prompt's UTF-16 code units, `cursor-task-<8 hex>` — computed
 *  identically by the sync (`cursor_task_id` in crates/fletch-core/src/agent/
 *  providers/cursor.rs), which tags the sub-agent's records with it. Keep the
 *  two in step: a drift here silently un-nests every replayed Cursor
 *  sub-agent. */
export function cursorTaskId(prompt: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < prompt.length; i += 1) {
    h = Math.imul(h ^ prompt.charCodeAt(i), 0x01000193);
  }
  return `cursor-task-${(h >>> 0).toString(16).padStart(8, "0")}`;
}

/** The replay id of the `n`-th (1-based) Task call in the main transcript whose
 *  BASE id is `base`: the base itself for the first, `<base>-<n>` after that.
 *  Numbering by base rather than by prompt text means even a 32-bit hash
 *  collision between different prompts still yields distinct rows. Mirrored by
 *  `suffixed` on the sync side, which derives the same ids so the two
 *  agree on which sub-agent file belongs to which call. */
export function cursorTaskIdNth(base: string, n: number): string {
  return n > 1 ? `${base}-${n}` : base;
}

export function normalizeTranscript(lines: unknown[]): RawEvent[] {
  const out: RawEvent[] = [];
  let toolSeq = 0;
  // Task calls seen so far in the main transcript, by base id. Without this,
  // two Tasks with identical prompts share an id and `upsertToolCall` merges
  // them into a single row carrying both sub-agents' turns.
  const taskSeq = new Map<string, number>();
  for (const line of lines) {
    const rec = asRecord(line);
    const role = rec.role;
    if (role !== "user" && role !== "assistant") continue; // drop unknown roles
    // Sub-agent records are tagged by the sync; a Task nested inside one is not
    // part of the main transcript's numbering and must not bump the count.
    const parent = typeof rec.parent_tool_use_id === "string" ? rec.parent_tool_use_id : undefined;
    const msg = asRecord(rec.message);
    const content = asBlockList(msg.content).map((b) => {
      if (b.type !== "tool_use" || b.id != null) return b;
      const prompt = b.name === "Task" ? asRecord(b.input).prompt : undefined;
      if (typeof prompt !== "string") return { ...b, id: `cursor-tool-${toolSeq++}` };
      // Trimmed because the sync hashes the sub-agent file's `<user_query>`,
      // which its envelope pads with newlines.
      const base = cursorTaskId(prompt.trim());
      if (parent !== undefined) return { ...b, id: base };
      const n = (taskSeq.get(base) ?? 0) + 1;
      taskSeq.set(base, n);
      return { ...b, id: cursorTaskIdNth(base, n) };
    });
    const ev: RawEvent = { type: role, message: { ...msg, content } };
    if (parent !== undefined) ev.parent_tool_use_id = parent;
    out.push(ev);
  }
  return out;
}
