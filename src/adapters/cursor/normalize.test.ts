import { describe, expect, it } from "vitest";
import type { ChatItem, RawEvent } from "@/adapters/types";
import { cursorAdapter } from "./index";

function render(lines: unknown[]): ChatItem[] {
  return cursorAdapter
    .normalizeTranscript(lines)
    .reduce<ChatItem[]>((acc, ev) => cursorAdapter.reduce(acc, ev as RawEvent), []);
}

// Cursor's on-disk transcript
// (~/.cursor/projects/<slug>/agent-transcripts/<id>/<id>.jsonl) is Claude-shaped
// content blocks (text / tool_use) but with `role` at the top level instead of
// `type`, and — unlike Claude — tool_use blocks carry NO `id` and there are NO
// tool_result rows (tool outputs aren't persisted).
const onDisk: unknown[] = [
  { role: "user", message: { content: [{ type: "text", text: "do it" }] } },
  {
    role: "assistant",
    message: {
      content: [
        { type: "text", text: "ok" },
        { type: "tool_use", name: "Glob", input: { glob_pattern: "**/*.ts" } },
        { type: "tool_use", name: "Read", input: { path: "a.ts" } },
      ],
    },
  },
];

describe("cursorAdapter.normalizeTranscript", () => {
  it("maps role→type and renders text + tool calls", () => {
    const items = render(onDisk);
    expect(items[0]).toEqual({ kind: "user_message", text: "do it" });
    expect(items[1]).toEqual({ kind: "agent_message", text: "ok", streaming: false });
  });

  it("synthesizes distinct ids so multiple id-less tool calls don't collapse", () => {
    const items = render(onDisk);
    const calls = items.filter((i) => i.kind === "tool_call") as Array<
      Extract<ChatItem, { kind: "tool_call" }>
    >;
    expect(calls.map((c) => [c.id, c.name])).toEqual([
      ["cursor-tool-0", "Glob"],
      ["cursor-tool-1", "Read"],
    ]);
  });

  it("is defensive against malformed lines", () => {
    expect(() =>
      cursorAdapter.normalizeTranscript([null, 1, {}, { role: "system" }]),
    ).not.toThrow();
    expect(cursorAdapter.normalizeTranscript([{ role: "system" }])).toEqual([]);
  });
});
