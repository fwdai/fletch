// OpenCode transcript replay.
//
// OpenCode's on-disk store is a blob store, not JSONL: message blobs
// (storage/message/<ses>/<msg>.json — role + metadata, no `type` field, no
// content) and part blobs (storage/part/<msg>/<part>.json — the content, each
// with a `type`). The Rust reader emits each message record followed by its
// part records, in order.
//
// We reassemble: a part's role comes from its parent message (messageID→role).
// A user message's typed text parts become one `user_message` event (the
// prompt is only in the transcript, never the live stream), stamped with the
// user message blob's record: that is where the backend positions the prompt
// (`opencode_prompt_texts` in crates/fletch-core/src/agent/providers/
// opencode.rs, which joins the same parts), and a turn row only lands on the
// bubble carrying that record. A `synthetic` text part (context opencode added
// itself, e.g. a mentioned file) is no part of the prompt and keeps a bubble
// of its own. Every other part maps its on-disk `type` to the live
// `{type, part}` event the reducer consumes (the part blob IS the live event's
// inner `part`).

import { asRecord } from "@/adapters/shared/json";
import { fromRecord } from "@/adapters/shared/record-seq";
import type { RawEvent } from "@/adapters/types";

// On-disk part type → live event type the reducer switches on.
const PART_TO_LIVE: Record<string, string> = {
  text: "text",
  reasoning: "reasoning",
  tool: "tool_use",
  "step-start": "step_start",
  "step-finish": "step_finish",
};

/** A text part the user typed: one opencode didn't mark `synthetic`. */
function isTypedText(rec: Record<string, unknown>): boolean {
  return rec.type === "text" && typeof rec.text === "string" && rec.synthetic !== true;
}

export function normalizeTranscript(
  lines: unknown[],
  seqs?: readonly number[],
  sessions?: readonly string[],
): RawEvent[] {
  // First pass: messageID → role (message blobs have role + id, no `type`).
  // Assistant message blobs also carry `modelID` (the model that produced the
  // turn); index it so the emitted text event can carry the model to the UI.
  // Also each message blob's index, and each message's typed text parts.
  const roleOf = new Map<string, string>();
  const modelOf = new Map<string, string>();
  const blobAt = new Map<string, number>();
  const typedOf = new Map<string, string[]>();
  for (const [i, line] of lines.entries()) {
    const rec = asRecord(line);
    if (rec.type == null && typeof rec.id === "string" && typeof rec.role === "string") {
      roleOf.set(rec.id, rec.role);
      blobAt.set(rec.id, i);
      if (typeof rec.modelID === "string") modelOf.set(rec.id, rec.modelID);
    } else if (isTypedText(rec) && typeof rec.messageID === "string") {
      const texts = typedOf.get(rec.messageID) ?? [];
      texts.push(rec.text as string);
      typedOf.set(rec.messageID, texts);
    }
  }

  const out: RawEvent[] = [];
  // User messages whose prompt has been emitted.
  const prompted = new Set<string>();
  for (const [i, line] of lines.entries()) {
    const rec = asRecord(line);
    if (typeof rec.type !== "string") continue; // message blob — role captured above
    const emit = (ev: RawEvent, at = i) => out.push(fromRecord(ev, seqs?.[at], sessions?.[at]));

    const messageID = typeof rec.messageID === "string" ? rec.messageID : undefined;
    const msgRole = messageID === undefined ? undefined : roleOf.get(messageID);

    if (rec.type === "text" && msgRole === "user" && messageID !== undefined) {
      if (!isTypedText(rec)) {
        emit({ type: "user_message", text: typeof rec.text === "string" ? rec.text : "" });
      } else if (!prompted.has(messageID)) {
        // The prompt, at its first typed part, on its message's record (known
        // here: the role came from it).
        prompted.add(messageID);
        const text = (typedOf.get(messageID) ?? []).join("\n");
        emit({ type: "user_message", text }, blobAt.get(messageID) ?? i);
      }
      continue;
    }

    const liveType = PART_TO_LIVE[rec.type];
    if (!liveType) continue; // subtask / unknown — nothing renderable
    const model = typeof rec.messageID === "string" ? modelOf.get(rec.messageID) : undefined;
    emit({ type: liveType, part: rec, model });
  }
  return out;
}
