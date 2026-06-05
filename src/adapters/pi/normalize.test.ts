import { describe, expect, it } from "vitest";

import { piAdapter } from "./index";
import type { ChatItem, RawEvent } from "../types";

// Renders the on-disk transcript the way re-attach will: normalizeTranscript
// (raw lines → RawEvent[]) then reduce (→ ChatItem[]).
function render(lines: unknown[]): ChatItem[] {
  return piAdapter
    .normalizeTranscript(lines)
    .reduce<ChatItem[]>((acc, ev) => piAdapter.reduce(acc, ev as RawEvent), []);
}

// Real persisted on-disk shape: `~/.pi/agent/sessions/<slug>/<ts>_<id>.jsonl`.
// Lines are `type:"message"` with a full message object (role user / assistant
// / toolResult), preceded by session/model_change/thinking_level_change. This
// is NOT the live `message_start/_update/_end` stream the reducer was built on,
// so normalizeTranscript translates: message → message_end, toolResult →
// tool_execution_end (reduce skips toolResult-role message_end).
const onDisk: unknown[] = [
  { type: "session", version: 3, id: "019e", cwd: "/x" },
  { type: "model_change", id: "m1", modelId: "claude-opus-4-8" },
  { type: "thinking_level_change", id: "t1", thinkingLevel: "medium" },
  {
    type: "message",
    id: "u1",
    message: { role: "user", content: [{ type: "text", text: "hi" }] },
  },
  {
    type: "message",
    id: "a1",
    message: {
      role: "assistant",
      content: [
        { type: "thinking", thinking: "pondering" },
        { type: "toolCall", id: "call-1", name: "bash", arguments: { command: "ls" } },
      ],
    },
  },
  {
    type: "message",
    id: "r1",
    message: {
      role: "toolResult",
      toolCallId: "call-1",
      toolName: "bash",
      content: [{ type: "text", text: "file.txt" }],
      isError: false,
    },
  },
  {
    type: "message",
    id: "a2",
    message: { role: "assistant", content: [{ type: "text", text: "done" }] },
  },
];

describe("piAdapter.normalizeTranscript", () => {
  it("renders on-disk transcript: drops preamble, maps messages + tool results", () => {
    const items = render(onDisk);
    expect(items).toEqual([
      { kind: "user_message", text: "hi" },
      { kind: "notice", subtype: "reasoning", text: "pondering" },
      {
        kind: "tool_call",
        id: "call-1",
        name: "bash",
        input: { command: "ls" },
        streaming: false,
      },
      { kind: "tool_result", tool_use_id: "call-1", content: "file.txt", is_error: false },
      { kind: "agent_message", text: "done" },
    ]);
  });

  it("does not throw on malformed lines (defensive)", () => {
    expect(() =>
      piAdapter.normalizeTranscript([null, 42, "x", {}, { type: "weird" }]),
    ).not.toThrow();
    expect(piAdapter.normalizeTranscript([{ type: "weird" }, {}])).toEqual([]);
  });
});
