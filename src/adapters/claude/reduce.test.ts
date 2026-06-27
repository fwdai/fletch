import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { ChatItem, RawEvent } from "../types";
import { claudeAdapter } from "./index";

const here = fileURLToPath(new URL(".", import.meta.url));

function readJsonl(name: string): unknown[] {
  const raw = readFileSync(join(here, "fixtures", name), "utf8");
  return raw
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l));
}

function reduceAll(events: RawEvent[]): ChatItem[] {
  return events.reduce<ChatItem[]>((acc, ev) => claudeAdapter.reduce(acc, ev), []);
}

describe("claudeAdapter.reduce — live events", () => {
  const events = readJsonl("live-events.jsonl") as RawEvent[];

  it("produces the expected normalized item list", () => {
    const items = reduceAll(events);
    expect(items).toEqual([
      { kind: "user_message", text: "hello" },
      { kind: "agent_message", text: "Hi there", streaming: false },
      {
        kind: "tool_call",
        id: "toolu_1",
        name: "Read",
        input: { path: "/tmp/x" },
      },
      {
        kind: "tool_result",
        tool_use_id: "toolu_1",
        content: "file body",
        is_error: false,
      },
      { kind: "notice", subtype: "turn_end", text: "success" },
    ]);
  });

  it("preserves streaming flag mid-stream", () => {
    // Stop after the first text delta — the assistant message should
    // still be marked streaming.
    const partial = events.slice(0, 3);
    const items = reduceAll(partial as RawEvent[]);
    const agent = items.find((i) => i.kind === "agent_message");
    expect(agent).toMatchObject({ text: "Hi there", streaming: true });
  });
});

describe("claudeAdapter — transcript replay", () => {
  const lines = readJsonl("transcript.jsonl");

  it("normalize → reduce produces a clean conversation with sanitized notices", () => {
    const events = claudeAdapter.normalizeTranscript(lines);
    const items = reduceAll(events);

    // user 'hello' → assistant 'Hi there' → slash_command notice
    // (the /login wrapper has no remaining user text) → user 'what's
    // next?' (the system-reminder is stripped) + hook_output notice →
    // assistant 'All set.'
    expect(items).toEqual([
      { kind: "user_message", text: "hello" },
      { kind: "agent_message", text: "Hi there", streaming: false },
      { kind: "notice", subtype: "slash_command", text: "/login" },
      { kind: "user_message", text: "what's next?" },
      { kind: "notice", subtype: "hook_output", text: "Hook stderr: x" },
      { kind: "agent_message", text: "All set.", streaming: false },
    ]);
  });

  it("drops unrelated transcript record kinds", () => {
    const events = claudeAdapter.normalizeTranscript([
      { type: "summary", summary: "ignored" },
      { type: "system", text: "ignored" },
    ]);
    expect(events).toEqual([]);
  });
});

describe("claudeAdapter.reduce — error result", () => {
  it("emits a notice with is_error=true", () => {
    const items = reduceAll([
      {
        type: "user",
        message: { role: "user", content: [{ type: "text", text: "go" }] },
      },
      {
        type: "result",
        subtype: "error_during_execution",
        is_error: true,
        result: "Boom",
      },
    ] as RawEvent[]);
    // Last two items: error notice, then turn_end notice.
    const errorNotice = items.find((it) => it.kind === "notice" && it.subtype === "error");
    expect(errorNotice).toMatchObject({
      kind: "notice",
      subtype: "error",
      is_error: true,
    });
  });
});

describe("claudeAdapter.reduce — unknown event", () => {
  it("returns prevItems unchanged", () => {
    const prev: ChatItem[] = [{ kind: "user_message", text: "x" }];
    const next = claudeAdapter.reduce(prev, { type: "future_event" } as RawEvent);
    expect(next).toBe(prev);
  });
});

describe("claudeAdapter.reduce — extended thinking", () => {
  // The thinking text arrives in the assistant event's `thinking` field
  // (shape confirmed against real persisted Claude events). The synthetic
  // blocks below mirror that real shape.
  it("captures a thinking block as a reasoning notice", () => {
    const items = reduceAll([
      {
        type: "assistant",
        message: {
          content: [
            { type: "thinking", thinking: "Let me reason…", signature: "s" },
            { type: "text", text: "Done." },
          ],
        },
      },
    ] as RawEvent[]);
    expect(items).toEqual([
      { kind: "notice", subtype: "reasoning", text: "Let me reason…" },
      { kind: "agent_message", text: "Done.", streaming: false },
    ]);
  });

  it("does not duplicate a thinking block already captured this turn", () => {
    const ev = {
      type: "assistant",
      message: {
        content: [{ type: "thinking", thinking: "same" }],
      },
    } as RawEvent;
    const once = reduceAll([ev]);
    const twice = reduceAll([ev, ev]);
    expect(twice).toEqual(once);
  });

  it("exposes a reasoning-visible policy", () => {
    expect(claudeAdapter.policy["notice:reasoning"]).toBe("show");
  });
});

describe("claudeAdapter.reduce — model", () => {
  it("stamps the model from a finalized assistant event onto the agent_message", () => {
    const items = reduceAll([
      {
        type: "assistant",
        message: {
          role: "assistant",
          model: "claude-opus-4-8",
          content: [{ type: "text", text: "Hi there" }],
        },
      },
    ] as RawEvent[]);
    expect(items).toEqual([
      {
        kind: "agent_message",
        text: "Hi there",
        streaming: false,
        model: "claude-opus-4-8",
      },
    ]);
  });

  it("stamps the model onto a message that streamed in before the finalized event", () => {
    const items = reduceAll([
      {
        type: "stream_event",
        event: {
          type: "content_block_start",
          content_block: { type: "text", text: "Hi there" },
        },
      },
      {
        type: "assistant",
        message: {
          role: "assistant",
          model: "claude-sonnet-4-6",
          content: [{ type: "text", text: "Hi there" }],
        },
      },
    ] as RawEvent[]);
    expect(items).toEqual([
      {
        kind: "agent_message",
        text: "Hi there",
        streaming: false,
        model: "claude-sonnet-4-6",
      },
    ]);
  });

  it("leaves model undefined when the event carries none", () => {
    const items = reduceAll([
      {
        type: "assistant",
        message: { role: "assistant", content: [{ type: "text", text: "Hi" }] },
      },
    ] as RawEvent[]);
    expect(items).toEqual([{ kind: "agent_message", text: "Hi", streaming: false }]);
  });
});

describe("claudeAdapter.reduce — subagent sidechain routing", () => {
  const spawn: RawEvent = {
    type: "assistant",
    message: {
      role: "assistant",
      content: [
        {
          type: "tool_use",
          id: "toolu_task",
          name: "Agent",
          input: { subagent_type: "Explore", description: "look", prompt: "go" },
        },
      ],
    },
  };

  it("nests sidechain events under the spawning tool_call, not the main log", () => {
    const items = reduceAll([
      spawn,
      // The subagent's own turns, tagged with the parent Task tool_use id.
      {
        type: "user",
        parent_tool_use_id: "toolu_task",
        message: { role: "user", content: "go" },
      },
      {
        type: "assistant",
        parent_tool_use_id: "toolu_task",
        message: { role: "assistant", content: [{ type: "text", text: "found it" }] },
      },
      // The Task's own result rides on a main-level user message (no parent).
      {
        type: "user",
        message: {
          role: "user",
          content: [{ type: "tool_result", tool_use_id: "toolu_task", content: "done" }],
        },
      },
    ] as RawEvent[]);

    // Main timeline holds only the tool_call and its result — no stray
    // user/agent bubbles from the subagent.
    expect(items.map((i) => i.kind)).toEqual(["tool_call", "tool_result"]);
    const call = items[0];
    expect(call.kind).toBe("tool_call");
    if (call.kind === "tool_call") {
      expect(call.children).toEqual([
        { kind: "user_message", text: "go" },
        { kind: "agent_message", text: "found it", streaming: false },
      ]);
    }
  });

  it("routes streaming tool_use deltas into the children slice", () => {
    const items = reduceAll([
      spawn,
      // The subagent's tool call streams in: content_block_start opens it,
      // input_json_delta fills its input — both tagged with the parent id, so
      // upsertToolCall / appendToolInputDelta must operate on the children
      // slice (positional index 0 there), not the main items list.
      {
        type: "stream_event",
        parent_tool_use_id: "toolu_task",
        event: {
          type: "content_block_start",
          content_block: { type: "tool_use", id: "toolu_child", name: "Read", input: "" },
        },
      },
      {
        type: "stream_event",
        parent_tool_use_id: "toolu_task",
        event: {
          type: "content_block_delta",
          index: 0,
          delta: { type: "input_json_delta", partial_json: '{"path":"/x"}' },
        },
      },
    ] as RawEvent[]);

    // Main timeline holds only the spawning Agent call.
    expect(items.map((i) => i.kind)).toEqual(["tool_call"]);
    const call = items[0];
    expect(call.kind).toBe("tool_call");
    if (call.kind === "tool_call") {
      expect(call.children).toEqual([
        {
          kind: "tool_call",
          id: "toolu_child",
          name: "Read",
          input: '{"path":"/x"}',
          streaming: true,
        },
      ]);
    }
  });

  it("drops a sidechain event whose parent tool_call hasn't arrived yet", () => {
    const items = reduceAll([
      {
        type: "user",
        parent_tool_use_id: "toolu_missing",
        message: { role: "user", content: "orphan" },
      },
    ] as RawEvent[]);
    expect(items).toEqual([]);
  });

  it("threads a nested subagent under its parent subagent", () => {
    const items = reduceAll([
      spawn,
      // The outer subagent spawns its own Task.
      {
        type: "assistant",
        parent_tool_use_id: "toolu_task",
        message: {
          role: "assistant",
          content: [{ type: "tool_use", id: "toolu_inner", name: "Agent", input: {} }],
        },
      },
      // The inner subagent's turn references the inner tool_use id.
      {
        type: "assistant",
        parent_tool_use_id: "toolu_inner",
        message: { role: "assistant", content: [{ type: "text", text: "deep" }] },
      },
    ] as RawEvent[]);

    const outer = items[0];
    expect(outer.kind).toBe("tool_call");
    if (outer.kind === "tool_call") {
      const inner = outer.children?.[0];
      expect(inner?.kind).toBe("tool_call");
      if (inner?.kind === "tool_call") {
        expect(inner.children).toEqual([{ kind: "agent_message", text: "deep", streaming: false }]);
      }
    }
  });
});
