import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { opencodeAdapter } from "@/adapters/opencode/index";
import type { ChatItem, RawEvent } from "@/adapters/types";

// Fixtures are real `opencode run --format json` output captured from
// opencode 1.15.12 (step / part event model — see ./reduce.ts).

const here = fileURLToPath(new URL(".", import.meta.url));

function readJsonl(name: string): unknown[] {
  return readFileSync(join(here, "fixtures", name), "utf8")
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l));
}

function run(events: RawEvent[]): ChatItem[] {
  return events.reduce<ChatItem[]>((acc, ev) => opencodeAdapter.reduce(acc, ev), []);
}

describe("opencodeAdapter", () => {
  it("reduces a real bash + message turn", () => {
    const items = run(readJsonl("sample.jsonl") as RawEvent[]);
    expect(items).toEqual([
      {
        kind: "tool_call",
        id: "call_00_AJ7I2XraqceVZ4eZm4Iv3927",
        name: "bash",
        input: { command: "echo hello", description: "Print hello to stdout" },
        streaming: false,
      },
      {
        kind: "tool_result",
        tool_use_id: "call_00_AJ7I2XraqceVZ4eZm4Iv3927",
        content: "hello\n",
        is_error: false,
      },
      { kind: "agent_message", text: "`hello`" },
      { kind: "notice", subtype: "turn_end", text: "success" },
    ]);
  });

  it("aliases OpenCode camelCase file fields to snake_case for the presenters", () => {
    const items = run(readJsonl("writeread.jsonl") as RawEvent[]);
    const calls = items.filter((i) => i.kind === "tool_call");
    // write call keeps its native fields AND gains the snake_case alias.
    expect(calls[0]).toMatchObject({
      name: "write",
      input: {
        filePath: "/private/tmp/oc-spike/note.txt",
        file_path: "/private/tmp/oc-spike/note.txt",
        content: "hi there",
      },
    });
    // read call gets the path alias too.
    expect(calls[1]).toMatchObject({
      name: "read",
      input: { file_path: "/private/tmp/oc-spike/note.txt" },
    });
  });

  it("marks a tool_call streaming until it completes", () => {
    const started = opencodeAdapter.reduce([], {
      type: "tool_use",
      part: {
        type: "tool",
        tool: "bash",
        callID: "x",
        state: { status: "running", input: { command: "ls" } },
      },
    } as RawEvent);
    expect(started).toEqual([
      { kind: "tool_call", id: "x", name: "bash", input: { command: "ls" }, streaming: true },
    ]);
  });

  it("flags a non-zero shell exit as an error", () => {
    const items = run([
      {
        type: "tool_use",
        part: {
          type: "tool",
          tool: "bash",
          callID: "y",
          state: {
            status: "completed",
            input: { command: "false" },
            output: "",
            metadata: { exit: 1 },
          },
        },
      },
    ] as RawEvent[]);
    const result = items.find((i) => i.kind === "tool_result");
    expect(result).toMatchObject({ tool_use_id: "y", is_error: true });
  });

  it("flags an errored tool as an error", () => {
    const items = run([
      {
        type: "tool_use",
        part: {
          type: "tool",
          tool: "read",
          callID: "z",
          state: { status: "error", input: { filePath: "/nope" }, output: "ENOENT" },
        },
      },
    ] as RawEvent[]);
    expect(items.find((i) => i.kind === "tool_result")).toMatchObject({
      tool_use_id: "z",
      is_error: true,
    });
  });

  it("ends the turn only on step_finish:stop, not intermediate tool-calls steps", () => {
    let items = run([{ type: "step_finish", part: { reason: "tool-calls" } }] as RawEvent[]);
    expect(items.find((i) => i.kind === "notice")).toBeUndefined();
    items = opencodeAdapter.reduce(items, {
      type: "step_finish",
      part: { reason: "stop" },
    } as RawEvent);
    expect(items.at(-1)).toEqual({ kind: "notice", subtype: "turn_end", text: "success" });
  });

  it("surfaces a top-level error event", () => {
    const items = run([{ type: "error", message: "boom" }] as RawEvent[]);
    expect(items).toEqual([{ kind: "notice", subtype: "error", text: "boom", is_error: true }]);
  });

  it("leaves prevItems untouched for step_start and unknown events", () => {
    const prev: ChatItem[] = [{ kind: "agent_message", text: "earlier" }];
    expect(opencodeAdapter.reduce(prev, { type: "step_start", part: {} } as RawEvent)).toBe(prev);
    expect(opencodeAdapter.reduce(prev, { type: "??" } as RawEvent)).toBe(prev);
  });

  it("exposes id and policy on the adapter", () => {
    expect(opencodeAdapter.id).toBe("opencode");
    expect(opencodeAdapter.policy["notice:turn_end"]).toBe("hide");
  });

  it("surfaces a reasoning part as a thinking notice (real --thinking output)", () => {
    const items = run(readJsonl("reasoning.jsonl") as RawEvent[]);
    const reasoning = items.find((i) => i.kind === "notice" && i.subtype === "reasoning");
    expect(reasoning).toBeDefined();
    expect((reasoning as { text: string }).text).toContain("12 times 8 = 96");
    expect(opencodeAdapter.policy["notice:reasoning"]).toBe("show");
  });
});
