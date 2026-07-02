import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { codexAdapter } from "@/adapters/codex/index";
import { applyPolicy } from "@/adapters/policy";
import type { ChatItem, RawEvent } from "@/adapters/types";

// Fixtures are real `codex exec --json` output captured from
// codex-cli 0.135.0 (thread / turn / item event model).

const here = fileURLToPath(new URL(".", import.meta.url));

function readJsonl(name: string): unknown[] {
  return readFileSync(join(here, "fixtures", name), "utf8")
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l));
}

function run(events: RawEvent[]): ChatItem[] {
  return events.reduce<ChatItem[]>((acc, ev) => codexAdapter.reduce(acc, ev), []);
}

describe("codexAdapter", () => {
  it("reduces a real command + message turn", () => {
    const items = run(readJsonl("sample.jsonl") as RawEvent[]);
    expect(items).toEqual([
      {
        kind: "tool_call",
        id: "item_0",
        name: "shell",
        input: "/bin/zsh -lc 'echo hello'",
        streaming: false,
      },
      {
        kind: "tool_result",
        tool_use_id: "item_0",
        content: "hello\n",
        is_error: false,
      },
      { kind: "agent_message", text: "It printed `hello`." },
      { kind: "notice", subtype: "turn_end", text: "success" },
    ]);
  });

  it("marks a tool_call streaming until its item.completed", () => {
    const started = codexAdapter.reduce([], {
      type: "item.started",
      item: { id: "x", type: "command_execution", command: "ls", status: "in_progress" },
    } as RawEvent);
    expect(started).toEqual([
      { kind: "tool_call", id: "x", name: "shell", input: "ls", streaming: true },
    ]);
  });

  it("renders an mcp_tool_call and flags failures", () => {
    const items = run([
      {
        type: "item.completed",
        item: {
          id: "item_0",
          type: "mcp_tool_call",
          server: "tasks",
          tool: "check_active",
          arguments: {},
          result: null,
          error: { message: "denied" },
          status: "failed",
        },
      },
    ] as RawEvent[]);
    expect(items).toEqual([
      { kind: "tool_call", id: "item_0", name: "tasks.check_active", input: {}, streaming: false },
      {
        kind: "tool_result",
        tool_use_id: "item_0",
        content: { message: "denied" },
        is_error: true,
      },
    ]);
  });

  it("flags non-zero shell exits as errors", () => {
    const items = run([
      {
        type: "item.completed",
        item: {
          id: "item_0",
          type: "command_execution",
          command: "false",
          aggregated_output: "",
          exit_code: 1,
          status: "completed",
        },
      },
    ] as RawEvent[]);
    const result = items.find((i) => i.kind === "tool_result");
    expect(result).toMatchObject({ tool_use_id: "item_0", is_error: true });
  });

  it("does not reset on a per-turn thread.started", () => {
    const prev: ChatItem[] = [{ kind: "agent_message", text: "earlier" }];
    expect(codexAdapter.reduce(prev, { type: "thread.started", thread_id: "t" } as RawEvent)).toBe(
      prev,
    );
  });

  it("replays a rollout transcript into the conversation", () => {
    const lines = readJsonl("rollout.jsonl");
    const events = codexAdapter.normalizeTranscript(lines);
    const items = run(events as RawEvent[]);
    expect(items).toEqual([
      { kind: "user_message", text: "run echo hello" },
      { kind: "tool_call", id: "call_1", name: "shell", input: "echo hello", streaming: false },
      {
        kind: "tool_result",
        tool_use_id: "call_1",
        content: "Process exited with code 0\nOutput:\nhello\n",
        is_error: false,
      },
      { kind: "agent_message", text: "It printed hello." },
      { kind: "notice", subtype: "turn_end", text: "success" },
    ]);
  });

  it("stamps the turn_context model onto replayed agent messages", () => {
    const events = codexAdapter.normalizeTranscript([
      { type: "turn_context", payload: { model: "gpt-5.2-codex" } },
      { type: "event_msg", payload: { type: "user_message", message: "hi" } },
      { type: "event_msg", payload: { type: "agent_message", message: "hello back" } },
      { type: "event_msg", payload: { type: "task_complete" } },
    ]);
    const items = run(events as RawEvent[]);
    expect(items).toContainEqual({
      kind: "agent_message",
      text: "hello back",
      model: "gpt-5.2-codex",
    });
  });

  it("replays a failed shell command as an error, not a success", () => {
    const events = codexAdapter.normalizeTranscript([
      {
        type: "response_item",
        payload: {
          type: "function_call",
          name: "exec_command",
          arguments: '{"cmd":"false"}',
          call_id: "c1",
        },
      },
      {
        type: "response_item",
        payload: {
          type: "function_call_output",
          call_id: "c1",
          output: "Process exited with code 1\nOutput:\n",
        },
      },
    ]);
    const items = run(events as RawEvent[]);
    const result = items.find((i) => i.kind === "tool_result");
    expect(result).toMatchObject({ tool_use_id: "c1", is_error: true });
  });

  it("renders a non-shell built-in tool by name, preserving its args", () => {
    const events = codexAdapter.normalizeTranscript([
      {
        type: "response_item",
        payload: {
          type: "function_call",
          name: "apply_patch",
          arguments: '{"patch":"diff..."}',
          call_id: "p1",
        },
      },
      {
        type: "response_item",
        payload: { type: "function_call_output", call_id: "p1", output: "applied" },
      },
    ]);
    const items = run(events as RawEvent[]);
    const call = items.find((i) => i.kind === "tool_call");
    // Not mislabeled "shell"; args preserved (not dropped).
    expect(call).toMatchObject({ name: "apply_patch", input: { patch: "diff..." } });
  });

  it("drops injected noise (response_item user/developer messages, reasoning)", () => {
    const lines = readJsonl("rollout.jsonl");
    const events = codexAdapter.normalizeTranscript(lines);
    // Only the clean event_msg user prompt survives, not the AGENTS.md /
    // permissions response_item messages.
    const userEvents = events.filter((e) => (e as RawEvent).type === "user");
    expect(userEvents).toHaveLength(1);
  });

  it("returns prevItems unchanged for unknown event types", () => {
    const prev: ChatItem[] = [{ kind: "user_message", text: "hi" }];
    expect(codexAdapter.reduce(prev, { type: "??" } as RawEvent)).toBe(prev);
  });

  it("exposes id and policy on the adapter", () => {
    expect(codexAdapter.id).toBe("codex");
    expect(codexAdapter.policy["notice:turn_end"]).toBe("hide");
  });

  it("surfaces reasoning as a thinking notice (not hidden by policy)", () => {
    const items = run([
      {
        type: "item.completed",
        item: { id: "r1", type: "reasoning", text: "Let me think…" },
      } as RawEvent,
    ]);
    expect(items).toContainEqual({
      kind: "notice",
      subtype: "reasoning",
      text: "Let me think…",
    });
    // And the display policy keeps it visible.
    const visible = applyPolicy(items, codexAdapter.policy);
    expect(visible).toContainEqual({
      kind: "notice",
      subtype: "reasoning",
      text: "Let me think…",
    });
  });
});
