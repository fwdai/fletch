// Unit tests for the fork-brief serializer. These exercise the pure prose
// assembly in isolation — the forkAgent path is what guarantees the input log
// is the record-derived, policy-filtered history with only matched turns
// overlaid (so pending turns and hidden items never reach here); this file pins
// the serialization and the cut at the anchor turn.

import { describe, expect, it } from "vitest";
import type { ChatItem } from "@/adapters";
import { APP_ACTION_PREFIX } from "@/delegation";
import { forkContextDigest, serializeForkItem } from "./forkDigest";

const user = (text: string, turnId?: string): ChatItem => ({
  kind: "user_message",
  text,
  ...(turnId ? { turnId } : {}),
});
const agent = (text: string): ChatItem => ({ kind: "agent_message", text });
const through = (turnId: string | null) => ({ kind: "through", turn_id: turnId }) as const;

describe("serializeForkItem", () => {
  it("renders user and agent messages", () => {
    expect(serializeForkItem(user("hello"))).toBe("User: hello");
    expect(serializeForkItem(agent("hi there"))).toBe("Assistant: hi there");
  });

  it("drops app-action (git delegation) user turns", () => {
    expect(serializeForkItem(user(`${APP_ACTION_PREFIX}open_pr`))).toBeNull();
  });

  it("drops empty agent messages", () => {
    expect(serializeForkItem(agent(""))).toBeNull();
  });

  it("serializes a tool call with its input", () => {
    const line = serializeForkItem({
      kind: "tool_call",
      id: "t1",
      name: "Bash",
      input: { command: "ls -la" },
    });
    expect(line).toContain("Assistant used tool `Bash`");
    expect(line).toContain('"command": "ls -la"');
  });

  it("flattens a subagent's nested conversation under the call", () => {
    const line = serializeForkItem({
      kind: "tool_call",
      id: "t1",
      name: "Agent",
      input: {},
      children: [user("sub prompt"), agent("sub answer")],
    });
    expect(line).toContain("Assistant used tool `Agent`");
    expect(line).toContain("User: sub prompt");
    expect(line).toContain("Assistant: sub answer");
  });

  it("serializes tool results, flagging errors", () => {
    expect(serializeForkItem({ kind: "tool_result", tool_use_id: "t1", content: "42 files" })).toBe(
      "Tool result:\n42 files",
    );
    expect(
      serializeForkItem({
        kind: "tool_result",
        tool_use_id: "t1",
        content: "boom",
        is_error: true,
      }),
    ).toBe("Tool error:\nboom");
  });

  it("flattens anthropic content-block arrays in tool results", () => {
    const line = serializeForkItem({
      kind: "tool_result",
      tool_use_id: "t1",
      content: [{ type: "text", text: "line one" }],
    });
    expect(line).toBe("Tool result:\nline one");
  });

  it("labels reasoning and error notices, passes others through", () => {
    expect(serializeForkItem({ kind: "notice", subtype: "reasoning", text: "let me think" })).toBe(
      "Assistant (thinking): let me think",
    );
    expect(serializeForkItem({ kind: "notice", subtype: "error", text: "it failed" })).toBe(
      "Error: it failed",
    );
    expect(serializeForkItem({ kind: "notice", subtype: "info", text: "fyi" })).toBe("fyi");
    expect(serializeForkItem({ kind: "notice", subtype: "turn_end", text: "" })).toBeNull();
  });

  it("never carries optimistic store-only queued messages", () => {
    expect(serializeForkItem({ kind: "queued_message", text: "later" })).toBeNull();
  });
});

describe("forkContextDigest", () => {
  it("carries nothing for a context-less fork", () => {
    expect(forkContextDigest([user("q"), agent("a")], { kind: "none" })).toBeNull();
  });

  it("returns null when the carried range has no prose", () => {
    expect(forkContextDigest([], through(null))).toBeNull();
    expect(forkContextDigest([user(`${APP_ACTION_PREFIX}x`)], through(null))).toBeNull();
  });

  it("joins the whole conversation when forking through the end, tool context included", () => {
    const log: ChatItem[] = [
      user("run the tests"),
      { kind: "tool_call", id: "t1", name: "Bash", input: { command: "npm test" } },
      { kind: "tool_result", tool_use_id: "t1", content: "3 failing", is_error: true },
      agent("two are flaky"),
    ];
    const digest = forkContextDigest(log, through(null));
    expect(digest).toBe(
      [
        "User: run the tests",
        'Assistant used tool `Bash`:\n{\n  "command": "npm test"\n}',
        "Tool error:\n3 failing",
        "Assistant: two are flaky",
      ].join("\n\n"),
    );
  });

  const conversation: ChatItem[] = [
    user("q0", "t0"),
    agent("a0"),
    user("q1", "t1"),
    agent("a1"),
    user("q2", "t2"),
  ];

  it("keeps history through the anchor turn's answer", () => {
    expect(forkContextDigest(conversation, through("t0"))).toBe("User: q0\n\nAssistant: a0");
    expect(forkContextDigest(conversation, through("t1"))).toBe(
      "User: q0\n\nAssistant: a0\n\nUser: q1\n\nAssistant: a1",
    );
  });

  it("carries the whole conversation through the last turn", () => {
    expect(forkContextDigest(conversation, through("t2"))).toBe(
      "User: q0\n\nAssistant: a0\n\nUser: q1\n\nAssistant: a1\n\nUser: q2",
    );
  });

  it("stops at the next turn of any kind, app actions included", () => {
    // App-action turns are ordinary turns to the backend cut, so the digest
    // must stop there too or it would brief the child on what it doesn't show.
    const log: ChatItem[] = [
      user("q0", "t0"),
      agent("a0"),
      user(`${APP_ACTION_PREFIX}open_pr`, "t-pr"),
      agent("pr opened"),
      user("q1", "t1"),
    ];
    expect(forkContextDigest(log, through("t0"))).toBe("User: q0\n\nAssistant: a0");
    expect(forkContextDigest(log, through("t-pr"))).toBe(
      "User: q0\n\nAssistant: a0\n\nAssistant: pr opened",
    );
  });

  it("does not let a user message without a turn row bound the cut", () => {
    // The backend cuts at the next *matched* turn; a message with no turn row
    // (typed into the native view) sits inside the carried range.
    const log: ChatItem[] = [user("q0", "t0"), agent("a0"), user("typed"), agent("a1")];
    expect(forkContextDigest(log, through("t0"))).toBe(
      "User: q0\n\nAssistant: a0\n\nUser: typed\n\nAssistant: a1",
    );
  });

  it("carries nothing when the anchor isn't in the log", () => {
    expect(forkContextDigest(conversation, through("gone"))).toBeNull();
  });
});
