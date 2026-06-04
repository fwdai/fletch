import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { cursorAdapter } from "./index";
import type { ChatItem, RawEvent } from "../types";

// Events captured from cursor-agent 2026.05.24
// (`-p --output-format stream-json`).

const here = fileURLToPath(new URL(".", import.meta.url));

function readJsonl(name: string): unknown[] {
  return readFileSync(join(here, "fixtures", name), "utf8")
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l));
}

function run(events: RawEvent[]): ChatItem[] {
  return events.reduce<ChatItem[]>((acc, ev) => cursorAdapter.reduce(acc, ev), []);
}

const SID = "daa224d7-a262-425d-8352-7c0eac21e5a0";

describe("cursorAdapter", () => {
  it("reduces a real shell-tool turn (delegating non-tool events to Claude)", () => {
    const items = run([
      { type: "system", subtype: "init", session_id: SID, model: "Composer 2.5 Fast" },
      {
        type: "user",
        message: { role: "user", content: [{ type: "text", text: "Run: echo hi" }] },
        session_id: SID,
      },
      {
        type: "tool_call",
        subtype: "started",
        call_id: "tool_x",
        tool_call: { shellToolCall: { args: { command: "echo hi" } } },
      },
      {
        type: "tool_call",
        subtype: "completed",
        call_id: "tool_x",
        tool_call: {
          shellToolCall: {
            args: { command: "echo hi" },
            result: { success: { exitCode: 0, stdout: "hi\n", interleavedOutput: "hi\n" } },
          },
        },
      },
      {
        type: "assistant",
        message: { role: "assistant", content: [{ type: "text", text: "It printed hi." }] },
        session_id: SID,
      },
      {
        type: "result",
        subtype: "success",
        is_error: false,
        result: "It printed hi.",
        session_id: SID,
      },
    ] as RawEvent[]);

    expect(items).toEqual([
      { kind: "user_message", text: "Run: echo hi" },
      { kind: "tool_call", id: "tool_x", name: "shell", input: "echo hi", streaming: false },
      { kind: "tool_result", tool_use_id: "tool_x", content: "hi\n", is_error: false },
      { kind: "agent_message", text: "It printed hi.", streaming: false },
      { kind: "notice", subtype: "turn_end", text: "success" },
    ]);
  });

  it("flags a non-zero shell exit as an error", () => {
    const items = run([
      {
        type: "tool_call",
        subtype: "completed",
        call_id: "t1",
        tool_call: {
          shellToolCall: {
            args: { command: "false" },
            result: { success: { exitCode: 1, stdout: "", interleavedOutput: "" } },
          },
        },
      },
    ] as RawEvent[]);
    const result = items.find((i) => i.kind === "tool_result");
    expect(result).toMatchObject({ tool_use_id: "t1", is_error: true });
  });

  it("marks a tool_call streaming until completion", () => {
    const items = run([
      {
        type: "tool_call",
        subtype: "started",
        call_id: "t2",
        tool_call: { shellToolCall: { args: { command: "ls" } } },
      },
    ] as RawEvent[]);
    expect(items).toEqual([
      { kind: "tool_call", id: "t2", name: "shell", input: "ls", streaming: true },
    ]);
  });

  // Cursor names file-tool args differently from Claude (globPattern,
  // targetDirectory, path, streamContent). The reducer aliases them to the
  // snake_case fields the shared presenters read, so glob/read/edit don't
  // render "(no pattern)"/"(no path)". Events captured from cursor-agent
  // 2026.06.03.
  it("aliases glob/read/edit tool args to the presenters' field names", () => {
    const items = run([
      {
        type: "tool_call",
        subtype: "completed",
        call_id: "g1",
        tool_call: {
          globToolCall: {
            args: { targetDirectory: "/repo/src", globPattern: "*.txt" },
            result: { success: { files: ["sample.txt"], totalFiles: 1 } },
          },
        },
      },
      {
        type: "tool_call",
        subtype: "completed",
        call_id: "r1",
        tool_call: {
          readToolCall: {
            args: { path: "/repo/src/sample.txt" },
            result: { success: { content: "hi\n", totalLines: 1 } },
          },
        },
      },
      {
        type: "tool_call",
        subtype: "completed",
        call_id: "e1",
        tool_call: {
          editToolCall: {
            args: { path: "/repo/src/note.md", streamContent: "bye" },
            result: { success: {} },
          },
        },
      },
    ] as RawEvent[]);

    const calls = items.filter((i) => i.kind === "tool_call");
    expect(calls).toEqual([
      {
        kind: "tool_call",
        id: "g1",
        name: "glob",
        input: {
          targetDirectory: "/repo/src",
          globPattern: "*.txt",
          pattern: "*.txt",
          path: "/repo/src",
        },
        streaming: false,
      },
      {
        kind: "tool_call",
        id: "r1",
        name: "read",
        input: { path: "/repo/src/sample.txt", file_path: "/repo/src/sample.txt" },
        streaming: false,
      },
      {
        kind: "tool_call",
        id: "e1",
        name: "edit",
        input: {
          path: "/repo/src/note.md",
          file_path: "/repo/src/note.md",
          streamContent: "bye",
          new_string: "bye",
        },
        streaming: false,
      },
    ]);
  });

  it("exposes id and reuses Claude's policy", () => {
    expect(cursorAdapter.id).toBe("cursor");
    expect(cursorAdapter.policy["notice:turn_end"]).toBe("hide");
    expect(cursorAdapter.normalizeTranscript([{ anything: true }])).toEqual([]);
  });

  it("accumulates `thinking` delta events into one reasoning notice (real output)", () => {
    // Cursor streams thinking as its own `thinking`/delta+completed events
    // (NOT a Claude content block), so it's handled in cursor's reducer.
    const items = run(readJsonl("reasoning.jsonl") as RawEvent[]);
    const reasoning = items.find(
      (i) => i.kind === "notice" && i.subtype === "reasoning",
    );
    expect(reasoning).toBeDefined();
    expect((reasoning as { text: string }).text).toBe(
      " I'm breaking down the multiplication using the distributive property—splitting 23 into 20 and 3, multiplying each part by 17, then adding the results to get 391.",
    );
    // The reasoning notice precedes the assistant's text answer.
    const rIdx = items.indexOf(reasoning as ChatItem);
    const aIdx = items.findIndex((i) => i.kind === "agent_message");
    expect(rIdx).toBeLessThan(aIdx);
    expect(cursorAdapter.policy["notice:reasoning"]).toBe("show");
  });

  it("does not leak a reasoning notice when there is no thinking", () => {
    const items = run([
      {
        type: "assistant",
        message: { role: "assistant", content: [{ type: "text", text: "hi" }] },
      },
    ] as RawEvent[]);
    expect(items.some((i) => i.kind === "notice" && i.subtype === "reasoning")).toBe(
      false,
    );
  });
});
