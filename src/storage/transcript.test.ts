import { describe, expect, it } from "vitest";

import { messageRowFor, messagesToChatItems } from "./transcript";
import type { MessageRow } from "./messages";
import type { ChatItem } from "../adapters";

// A row as it comes back from the DB (id/created_at filled by sqlite).
function row(r: ReturnType<typeof messageRowFor>, i: number): MessageRow {
  return { id: `m${i}`, created_at: i, ...r };
}

describe("transcript round-trip", () => {
  it("survives serialize → deserialize for every item kind", () => {
    const items: ChatItem[] = [
      { kind: "user_message", text: "run echo hi" },
      { kind: "tool_call", id: "t1", name: "shell", input: "echo hi" },
      { kind: "tool_result", tool_use_id: "t1", content: "hi\n", is_error: false },
      { kind: "tool_result", tool_use_id: "t2", content: { error: "boom" }, is_error: true },
      { kind: "agent_message", text: "It printed hi." },
      { kind: "notice", subtype: "turn_end", text: "success" },
    ];

    const rows = items.map((it, i) => row(messageRowFor(it, i, "agent-1"), i));
    expect(messagesToChatItems(rows)).toEqual(items);
  });

  it("preserves object tool inputs/results, not just strings", () => {
    const items: ChatItem[] = [
      { kind: "tool_call", id: "x", name: "edit", input: { file: "a.ts", line: 3 } },
    ];
    const rows = items.map((it, i) => row(messageRowFor(it, i, "a"), i));
    const back = messagesToChatItems(rows);
    expect(back[0]).toEqual({
      kind: "tool_call",
      id: "x",
      name: "edit",
      input: { file: "a.ts", line: 3 },
    });
  });
});
