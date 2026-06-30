// OpenCode transcript replay.
//
// OpenCode's on-disk store is a blob store, not JSONL: message blobs
// (storage/message/<ses>/<msg>.json — role + metadata, no `type` field, no
// content) and part blobs (storage/part/<msg>/<part>.json — the content, each
// with a `type`). The Rust reader emits each message record followed by its
// part records, in order.
//
// We reassemble: a part's role comes from its parent message (messageID→role).
// A user message's text part becomes a `user_message` event (the prompt is
// only in the transcript, never the live stream). Every other part maps its
// on-disk `type` to the live `{type, part}` event the reducer consumes
// (the part blob IS the live event's inner `part`).

import { asRecord } from "@/adapters/shared/json";
import type { RawEvent } from "@/adapters/types";

// On-disk part type → live event type the reducer switches on.
const PART_TO_LIVE: Record<string, string> = {
  text: "text",
  reasoning: "reasoning",
  tool: "tool_use",
  "step-start": "step_start",
  "step-finish": "step_finish",
};

export function normalizeTranscript(lines: unknown[]): RawEvent[] {
  // First pass: messageID → role (message blobs have role + id, no `type`).
  // Assistant message blobs also carry `modelID` (the model that produced the
  // turn); index it so the emitted text event can carry the model to the UI.
  const roleOf = new Map<string, string>();
  const modelOf = new Map<string, string>();
  for (const line of lines) {
    const rec = asRecord(line);
    if (rec.type == null && typeof rec.id === "string" && typeof rec.role === "string") {
      roleOf.set(rec.id, rec.role);
      if (typeof rec.modelID === "string") modelOf.set(rec.id, rec.modelID);
    }
  }

  const out: RawEvent[] = [];
  for (const line of lines) {
    const rec = asRecord(line);
    if (typeof rec.type !== "string") continue; // message blob — role captured above

    const msgRole = typeof rec.messageID === "string" ? roleOf.get(rec.messageID) : undefined;

    // A user message's text is the prompt.
    if (rec.type === "text" && msgRole === "user") {
      out.push({ type: "user_message", text: typeof rec.text === "string" ? rec.text : "" });
      continue;
    }

    const liveType = PART_TO_LIVE[rec.type];
    if (!liveType) continue; // subtask / unknown — nothing renderable
    const model = typeof rec.messageID === "string" ? modelOf.get(rec.messageID) : undefined;
    out.push({ type: liveType, part: rec, model });
  }
  return out;
}
