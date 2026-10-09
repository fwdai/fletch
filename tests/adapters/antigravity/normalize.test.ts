import { describe, expect, it } from "vitest";
import { antigravityAdapter } from "@/adapters/antigravity/index";
import type { ChatItem, RawEvent } from "@/adapters/types";

function render(lines: unknown[], seqs?: number[]): ChatItem[] {
  return antigravityAdapter
    .normalizeTranscript(lines, seqs)
    .reduce<ChatItem[]>((acc, ev) => antigravityAdapter.reduce(acc, ev as RawEvent), []);
}

// agy's on-disk transcript_full.jsonl steps. USER_INPUT wraps the prompt in
// <USER_REQUEST>; PLANNER_RESPONSE carries either assistant `content` (markdown
// text) or `tool_calls`; result steps (LIST_DIRECTORY/RUN_COMMAND/…, open-ended)
// carry `content`. Tool calls and results share no id, so they pair by order.
const transcript: unknown[] = [
  {
    step_index: 0,
    type: "USER_INPUT",
    content:
      "<USER_REQUEST>\nlist the files\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\ntime: x\n</ADDITIONAL_METADATA>",
  },
  { step_index: 1, type: "CONVERSATION_HISTORY" },
  {
    step_index: 2,
    type: "PLANNER_RESPONSE",
    tool_calls: [{ name: "list_dir", args: { DirectoryPath: '"/x"' } }],
  },
  { step_index: 3, type: "LIST_DIRECTORY", content: "a.ts\nb.ts" },
  { step_index: 4, type: "PLANNER_RESPONSE", content: "Here are the files." },
];

/** Session-record seqs for `lines`, offset so they can't pass for indices. */
const seqsFor = (lines: unknown[]) => lines.map((_, i) => 100 + i);

describe("antigravityAdapter.normalizeTranscript", () => {
  it("stamps every item with the seq of the record it came from", () => {
    expect(render(transcript, seqsFor(transcript)).map((i) => [i.kind, i.recordSeq])).toEqual([
      ["user_message", 100],
      ["tool_call", 102],
      ["tool_result", 103],
      ["agent_message", 104],
    ]);
  });

  it("renders user input, an order-paired tool call+result, and assistant text", () => {
    const items = render(transcript);
    expect(items[0]).toEqual({ kind: "user_message", text: "list the files" });

    const call = items.find((i) => i.kind === "tool_call") as
      | Extract<ChatItem, { kind: "tool_call" }>
      | undefined;
    const result = items.find((i) => i.kind === "tool_result") as
      | Extract<ChatItem, { kind: "tool_result" }>
      | undefined;
    expect(call?.name).toBe("list_dir");
    expect(result?.content).toBe("a.ts\nb.ts");
    // The result is paired to the preceding call (no shared id on disk).
    expect(result?.tool_use_id).toBe(call?.id);

    expect(items[items.length - 1]).toEqual({
      kind: "agent_message",
      text: "Here are the files.",
    });
  });

  it("is defensive against malformed lines", () => {
    expect(() =>
      antigravityAdapter.normalizeTranscript([null, 1, {}, { type: "WEIRD" }]),
    ).not.toThrow();
    // CONVERSATION_HISTORY + unknown-without-content drop to nothing.
    expect(antigravityAdapter.normalizeTranscript([{ type: "CONVERSATION_HISTORY" }])).toEqual([]);
  });
});
