import { describe, expect, it } from "vitest";
import { opencodeAdapter } from "@/adapters/opencode/index";
import type { ChatItem, RawEvent } from "@/adapters/types";

function render(lines: unknown[], seqs?: number[]): ChatItem[] {
  return opencodeAdapter
    .normalizeTranscript(lines, seqs)
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

/** Session-record seqs for `lines`, offset so they can't pass for indices. */
const seqsFor = (lines: unknown[]) => lines.map((_, i) => 100 + i);

describe("opencodeAdapter.normalizeTranscript", () => {
  it("stamps every item with the seq of the record it came from", () => {
    // Message blobs emit nothing, but the prompt carries its user message's
    // record, where the backend positions it; the completed tool part fans out
    // into call + result.
    expect(render(records, seqsFor(records)).map((i) => [i.kind, i.recordSeq])).toEqual([
      ["user_message", 100],
      ["agent_message", 103],
      ["tool_call", 104],
      ["tool_result", 104],
      ["notice", 105],
    ]);
  });

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

  it("draws a prompt's typed parts as one bubble on its user message, as the backend reads it", () => {
    // `opencode_prompt_texts`: the message blob, with its non-synthetic text
    // parts joined by newlines.
    const lines: unknown[] = [
      { id: "m1", role: "user", sessionID: "s" },
      { id: "p1", type: "text", messageID: "m1", text: "fix it" },
      { id: "p2", type: "text", messageID: "m1", synthetic: true, text: "Called the Read tool" },
      { id: "p3", type: "text", messageID: "m1", text: "and test it" },
      { id: "m2", role: "assistant", sessionID: "s" },
      { id: "p4", type: "text", messageID: "m2", text: "done" },
    ];
    expect(
      render(lines, seqsFor(lines)).map((i) => [i.kind, "text" in i && i.text, i.recordSeq]),
    ).toEqual([
      ["user_message", "fix it\nand test it", 100],
      ["user_message", "Called the Read tool", 102],
      ["agent_message", "done", 105],
    ]);
  });

  it("is defensive against malformed records", () => {
    expect(() =>
      opencodeAdapter.normalizeTranscript([null, 7, {}, { type: "subtask" }]),
    ).not.toThrow();
  });
});
