import { describe, expect, it } from "vitest";
import type { SessionRecord } from "@/api";
import { reduceRecords } from "@/helpers";

// Canonical session_records hold verbatim per-provider transcript bodies.
// reduceRecords renders them the same way on-disk replay does:
// normalizeTranscript → reduce.
function rec(body: Record<string, unknown>, provider = "pi"): SessionRecord {
  return {
    seq: 0,
    provider,
    source: "transcript",
    native_id: "x",
    agent_version: null,
    body,
  };
}

describe("reduceRecords", () => {
  it("renders Pi on-disk records via normalizeTranscript + reduce", () => {
    const records = [
      rec({ type: "session", id: "s" }),
      rec({ type: "message", message: { role: "user", content: [{ type: "text", text: "hi" }] } }),
      rec({
        type: "message",
        message: { role: "assistant", content: [{ type: "text", text: "yo" }] },
      }),
    ];
    expect(reduceRecords("pi", records)).toEqual([
      { kind: "user_message", text: "hi" },
      { kind: "agent_message", text: "yo" },
    ]);
  });

  it("nests persisted Claude sub-agent records under the spawning tool call", () => {
    // The sync ingests a sub-agent's transcript file with a top-level
    // `parent_tool_use_id` (the live-wire shape), appended after the parent's
    // tool_use record. Replay must thread those under the tool_call's
    // `children`, not the main timeline — the same as the live render.
    const sub = { isSidechain: true, agentId: "abc", parent_tool_use_id: "toolu_task" };
    const records = [
      rec(
        {
          type: "assistant",
          uuid: "m1",
          message: {
            role: "assistant",
            content: [
              { type: "tool_use", id: "toolu_task", name: "Agent", input: { prompt: "look" } },
            ],
          },
        },
        "claude",
      ),
      rec(
        { type: "user", uuid: "s1", ...sub, message: { role: "user", content: "look" } },
        "claude",
      ),
      rec(
        {
          type: "assistant",
          uuid: "s2",
          ...sub,
          message: { role: "assistant", content: [{ type: "text", text: "found it" }] },
        },
        "claude",
      ),
      rec(
        {
          type: "user",
          uuid: "m2",
          toolUseResult: { status: "completed", agentId: "abc" },
          message: {
            role: "user",
            content: [{ type: "tool_result", tool_use_id: "toolu_task", content: "found it" }],
          },
        },
        "claude",
      ),
    ];
    const items = reduceRecords("claude", records);
    expect(items.map((i) => i.kind)).toEqual(["tool_call", "tool_result"]);
    expect(items[0]).toMatchObject({
      kind: "tool_call",
      id: "toolu_task",
      children: [
        { kind: "user_message", text: "look" },
        { kind: "agent_message", text: "found it" },
      ],
    });
  });

  it("nests persisted Codex sub-agent records under the spawning Agent call", () => {
    // The sync ingests the child rollout's lines after the parent's, each
    // tagged with the spawn `call_id` (the id of the parent's
    // `SubAgentActivity` / `started` item). Shapes from codex-cli 0.153.4.
    const spawn = "call_spawn";
    const records = [
      rec(
        {
          type: "response_item",
          payload: {
            type: "function_call",
            name: "spawn_agent",
            namespace: "collaboration",
            arguments: '{"task_name":"review","fork_turns":"all","message":"gAAAAABq=="}',
            call_id: spawn,
          },
        },
        "codex",
      ),
      rec(
        {
          type: "response_item",
          payload: {
            type: "function_call_output",
            call_id: spawn,
            output: '{"task_name":"/root/review"}',
          },
        },
        "codex",
      ),
      rec(
        {
          type: "event_msg",
          parent_tool_use_id: spawn,
          payload: {
            type: "item_completed",
            item: { type: "AgentMessage", id: "m1", content: [{ type: "text", text: "LGTM" }] },
          },
        },
        "codex",
      ),
    ];
    const items = reduceRecords("codex", records);
    expect(items.map((i) => i.kind)).toEqual(["tool_call", "tool_result"]);
    expect(items[0]).toMatchObject({
      kind: "tool_call",
      id: spawn,
      name: "Agent",
      input: { description: "review", subagent_type: "/root/review" },
      children: [{ kind: "agent_message", text: "LGTM" }],
    });
  });

  it("is defensive against malformed bodies", () => {
    const records = [rec(null as unknown as Record<string, unknown>), rec({ type: "weird" })];
    expect(() => reduceRecords("pi", records)).not.toThrow();
    expect(reduceRecords("pi", records)).toEqual([]);
  });
});
