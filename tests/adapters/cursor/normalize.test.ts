import { describe, expect, it } from "vitest";
import { cursorAdapter } from "@/adapters/cursor/index";
import { cursorTaskId, cursorTaskIdNth } from "@/adapters/cursor/normalize";
import type { ChatItem, RawEvent } from "@/adapters/types";

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

  // A Task block gets its id from its prompt — the same id the sync tags the
  // sub-agent's records with (`cursor_task_id` in providers/cursor.rs) — so the
  // Claude reducer nests those records under the call. Other blocks keep the
  // positional ids.
  it("gives a Task block a prompt-derived id and nests its tagged sub-agent records", () => {
    const prompt = "Explore src/ for seams.\n\nReport back.";
    const taskId = cursorTaskId(prompt);
    const items = render([
      {
        role: "assistant",
        message: {
          content: [
            { type: "tool_use", name: "Read", input: { path: "a.ts" } },
            {
              type: "tool_use",
              name: "Task",
              input: { description: "Find seams", subagent_type: "explore", prompt },
            },
          ],
        },
      },
      // The sub-agent's own file, ingested with the tag. Its user turn wears
      // Cursor's envelope, which the shared sanitizer strips.
      {
        role: "user",
        parent_tool_use_id: taskId,
        message: {
          content: [
            {
              type: "text",
              text: `<timestamp>Fri</timestamp>\n<user_query>\n${prompt}\n</user_query>`,
            },
          ],
        },
      },
      {
        role: "assistant",
        parent_tool_use_id: taskId,
        message: {
          content: [
            { type: "tool_use", name: "Grep", input: { pattern: "emit" } },
            { type: "text", text: "Two seams." },
          ],
        },
      },
      { type: "turn_ended", status: "success", parent_tool_use_id: taskId },
      { role: "assistant", message: { content: [{ type: "text", text: "Thanks." }] } },
    ]);
    expect(items).toEqual([
      { kind: "tool_call", id: "cursor-tool-0", name: "Read", input: { path: "a.ts" } },
      {
        kind: "tool_call",
        id: taskId,
        name: "Task",
        input: { description: "Find seams", subagent_type: "explore", prompt },
        children: [
          { kind: "user_message", text: prompt },
          { kind: "tool_call", id: "cursor-tool-1", name: "Grep", input: { pattern: "emit" } },
          { kind: "agent_message", text: "Two seams.", streaming: false },
        ],
      },
      { kind: "agent_message", text: "Thanks.", streaming: false },
    ]);
  });

  // Two Tasks with the same prompt hash the same. Without an occurrence
  // suffix they share an id, `upsertToolCall` merges them into one row, and
  // both sub-agents' turns land under it.
  it("numbers repeated Task prompts so they don't collapse into one call", () => {
    const prompt = "look at a.rs";
    const base = cursorTaskId(prompt);
    const task = {
      role: "assistant",
      message: {
        content: [{ type: "tool_use", name: "Task", input: { prompt } }],
      },
    };
    const items = render([
      task,
      // A nested Task inside a sub-agent's own (tagged) record: not part of
      // the main transcript's numbering, so it must not bump the count.
      { ...task, parent_tool_use_id: base },
      task,
    ]);
    const calls = items.filter((i) => i.kind === "tool_call") as Array<
      Extract<ChatItem, { kind: "tool_call" }>
    >;
    expect(calls.map((c) => c.id)).toEqual([base, `${base}-2`]);
    expect(calls[0].children).toEqual([
      { kind: "tool_call", id: base, name: "Task", input: { prompt } },
    ]);
  });

  it("derives a stable Task id that matches the sync's (FNV-1a over UTF-16 units)", () => {
    // Pinned against `task_id_is_a_stable_fnv1a_of_the_prompt` in
    // crates/fletch-core/src/agent/providers/cursor.rs.
    expect(cursorTaskId("")).toBe("cursor-task-811c9dc5");
    expect(cursorTaskId("look")).toBe(cursorTaskId("look"));
    expect(cursorTaskId("look")).not.toBe(cursorTaskId("look "));
    expect(cursorTaskId("look")).toMatch(/^cursor-task-[0-9a-f]{8}$/);
    // …and the occurrence suffix, pinned against `suffixed` in providers/cursor.rs.
    expect(cursorTaskIdNth("cursor-task-811c9dc5", 1)).toBe("cursor-task-811c9dc5");
    expect(cursorTaskIdNth("cursor-task-811c9dc5", 2)).toBe("cursor-task-811c9dc5-2");
  });

  it("is defensive against malformed lines", () => {
    expect(() =>
      cursorAdapter.normalizeTranscript([null, 1, {}, { role: "system" }]),
    ).not.toThrow();
    expect(cursorAdapter.normalizeTranscript([{ role: "system" }])).toEqual([]);
  });
});
