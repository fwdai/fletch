import { describe, expect, it } from "vitest";
import { opencodeAdapter } from "@/adapters/opencode/index";
import type { ChatItem, RawEvent } from "@/adapters/types";

function render(lines: unknown[]): ChatItem[] {
  return opencodeAdapter
    .normalizeTranscript(lines)
    .reduce<ChatItem[]>((acc, ev) => opencodeAdapter.reduce(acc, ev as RawEvent), []);
}

// OpenCode's on-disk store is a blob store, not JSONL: message blobs
// (storage/message/<ses>/<msg>.json — role + metadata, NO `type` field, NO
// content) and part blobs (storage/part/<msg>/<part>.json — the content, with a
// `type`). The Rust reader emits each message record then its part records, in
// order. normalizeTranscript reassembles: a part's role comes from its parent
// message (messageID→role); user text parts become user_message, everything
// else maps part.type → the live `{type, part}` event the reducer consumes.
const records: unknown[] = [
  { id: "m1", role: "user", sessionID: "s" }, // message blob (no `type`)
  { id: "p1", type: "text", messageID: "m1", text: "hello" },
  { id: "m2", role: "assistant", sessionID: "s", modelID: "grok-code" },
  { id: "p2", type: "text", messageID: "m2", text: "hi there" },
  {
    id: "p3",
    type: "tool",
    messageID: "m2",
    callID: "c1",
    tool: "bash",
    state: { status: "completed", input: { command: "ls" }, output: "file.txt" },
  },
  { id: "p4", type: "step-finish", messageID: "m2", reason: "stop" },
];

describe("opencodeAdapter.normalizeTranscript", () => {
  it("reassembles message+part blobs into a rendered conversation", () => {
    const items = render(records);
    expect(items).toEqual([
      { kind: "user_message", text: "hello" },
      // The assistant blob's modelID rides through onto the agent_message.
      { kind: "agent_message", text: "hi there", model: "grok-code" },
      { kind: "tool_call", id: "c1", name: "bash", input: { command: "ls" }, streaming: false },
      { kind: "tool_result", tool_use_id: "c1", content: "file.txt", is_error: false },
      { kind: "notice", subtype: "turn_end", text: "success" },
    ]);
  });

  it("is defensive against malformed records", () => {
    expect(() =>
      opencodeAdapter.normalizeTranscript([null, 7, {}, { type: "subtask" }]),
    ).not.toThrow();
  });
});
