import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { ChatItem, RawEvent } from "../types";
import { piAdapter } from "./index";

// Fixtures are real `pi -p --mode json` output captured from pi 0.74.2
// (@earendil-works/pi-coding-agent — see ./reduce.ts).

const here = fileURLToPath(new URL(".", import.meta.url));

function readJsonl(name: string): unknown[] {
  return readFileSync(join(here, "fixtures", name), "utf8")
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l));
}

function run(events: RawEvent[]): ChatItem[] {
  return events.reduce<ChatItem[]>((acc, ev) => piAdapter.reduce(acc, ev), []);
}

describe("piAdapter", () => {
  it("reduces a real bash + message turn", () => {
    const items = run(readJsonl("sample.jsonl") as RawEvent[]);
    // Shape: user echo, the bash tool call + its result, the answer, turn end.
    expect(items.map((i) => i.kind)).toEqual([
      "user_message",
      "tool_call",
      "tool_result",
      "agent_message",
      "notice",
    ]);

    expect(items[0]).toEqual({
      kind: "user_message",
      text: "Run the shell command: echo hello, then tell me what it printed in one short sentence.",
    });

    const call = items[1];
    expect(call).toMatchObject({
      kind: "tool_call",
      name: "bash",
      input: { command: "echo hello" },
      streaming: false,
    });

    // Result is keyed back to the call and carries the flattened output.
    expect(items[2]).toEqual({
      kind: "tool_result",
      tool_use_id: (call as { id: string }).id,
      content: "hello\n",
      is_error: false,
    });

    // The agent_message carries the model pi reports on the message_end event.
    expect(items[3]).toEqual({
      kind: "agent_message",
      text: 'It printed "hello".',
      model: "claude-opus-4-7",
    });
    expect(items[4]).toEqual({ kind: "notice", subtype: "turn_end", text: "success" });
  });

  it("aliases Pi's `path` arg to `file_path` for the presenters", () => {
    const items = run(readJsonl("write.jsonl") as RawEvent[]);
    const write = items.find((i) => i.kind === "tool_call" && i.name === "write");
    expect(write).toMatchObject({
      input: { path: "note.txt", file_path: "note.txt", content: "hi-there" },
    });
  });

  it("marks a tool_call from a finalized assistant message (not streaming)", () => {
    const items = piAdapter.reduce([], {
      type: "message_end",
      message: {
        role: "assistant",
        content: [{ type: "toolCall", id: "t1", name: "bash", arguments: { command: "ls" } }],
      },
    } as RawEvent);
    expect(items).toEqual([
      { kind: "tool_call", id: "t1", name: "bash", input: { command: "ls" }, streaming: false },
    ]);
  });

  it("flags an errored tool result", () => {
    const items = run([
      {
        type: "tool_execution_end",
        toolCallId: "z",
        toolName: "bash",
        result: { content: [{ type: "text", text: "boom" }] },
        isError: true,
      },
    ] as RawEvent[]);
    expect(items).toEqual([
      { kind: "tool_result", tool_use_id: "z", content: "boom", is_error: true },
    ]);
  });

  it("ends the turn on agent_end, not the per-step turn_end", () => {
    const afterTurnEnd = run([{ type: "turn_end" }] as RawEvent[]);
    expect(afterTurnEnd.find((i) => i.kind === "notice")).toBeUndefined();
    const afterAgentEnd = piAdapter.reduce(afterTurnEnd, { type: "agent_end" } as RawEvent);
    expect(afterAgentEnd.at(-1)).toEqual({
      kind: "notice",
      subtype: "turn_end",
      text: "success",
    });
  });

  it("does not reset or render on session / housekeeping events", () => {
    const prev: ChatItem[] = [{ kind: "agent_message", text: "earlier" }];
    expect(piAdapter.reduce(prev, { type: "session", id: "x" } as RawEvent)).toBe(prev);
    expect(piAdapter.reduce(prev, { type: "agent_start" } as RawEvent)).toBe(prev);
    expect(piAdapter.reduce(prev, { type: "??" } as RawEvent)).toBe(prev);
  });

  it("exposes id and policy on the adapter", () => {
    expect(piAdapter.id).toBe("pi");
    expect(piAdapter.policy["notice:turn_end"]).toBe("hide");
  });

  it("captures a thinking block as a reasoning notice (real --thinking output)", () => {
    const items = run(readJsonl("reasoning.jsonl") as RawEvent[]);
    const reasoning = items.find((i) => i.kind === "notice" && i.subtype === "reasoning");
    expect(reasoning).toBeDefined();
    expect((reasoning as { text: string }).text).toContain("12 times 8 equals 96");
    expect(piAdapter.policy["notice:reasoning"]).toBe("show");
  });
});
