// Unit tests for the fork-brief serializer. These exercise the pure prose
// assembly in isolation — the forkAgent path is what guarantees the input log
// is the record-only, policy-filtered surface the child renders (so pending
// turns and hidden items never reach here); this file pins the serialization
// and the up_to_message cutoff.

import { describe, expect, it } from "vitest";
import type { ChatItem } from "@/adapters";
import { APP_ACTION_PREFIX } from "@/components/RightPanel/delegation";
import { forkContextDigest, serializeForkItem } from "./forkDigest";

const user = (text: string): ChatItem => ({ kind: "user_message", text });
const agent = (text: string): ChatItem => ({ kind: "agent_message", text });

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
    expect(forkContextDigest([], { kind: "full" })).toBeNull();
    expect(forkContextDigest([user(`${APP_ACTION_PREFIX}x`)], { kind: "full" })).toBeNull();
  });

  it("joins the full conversation, including tool context", () => {
    const log: ChatItem[] = [
      user("run the tests"),
      { kind: "tool_call", id: "t1", name: "Bash", input: { command: "npm test" } },
      { kind: "tool_result", tool_use_id: "t1", content: "3 failing", is_error: true },
      agent("two are flaky"),
    ];
    const digest = forkContextDigest(log, { kind: "full" });
    expect(digest).toBe(
      [
        "User: run the tests",
        'Assistant used tool `Bash`:\n{\n  "command": "npm test"\n}',
        "Tool error:\n3 failing",
        "Assistant: two are flaky",
      ].join("\n\n"),
    );
  });

  // Navigable prompt ordinals (0-based, git actions excluded). up_to_message
  // stops just before the prompt that follows the selected ordinal.
  const conversation: ChatItem[] = [user("q0"), agent("a0"), user("q1"), agent("a1"), user("q2")];

  it("up_to_message keeps history through the selected prompt's answer", () => {
    expect(forkContextDigest(conversation, { kind: "up_to_message", prompt: 0 })).toBe(
      "User: q0\n\nAssistant: a0",
    );
    expect(forkContextDigest(conversation, { kind: "up_to_message", prompt: 1 })).toBe(
      "User: q0\n\nAssistant: a0\n\nUser: q1\n\nAssistant: a1",
    );
  });

  it("up_to_message on the last prompt carries the whole conversation", () => {
    expect(forkContextDigest(conversation, { kind: "up_to_message", prompt: 2 })).toBe(
      "User: q0\n\nAssistant: a0\n\nUser: q1\n\nAssistant: a1\n\nUser: q2",
    );
  });

  it("skips app-action turns when counting the ordinal", () => {
    const log: ChatItem[] = [
      user("q0"),
      user(`${APP_ACTION_PREFIX}open_pr`),
      agent("a0"),
      user("q1"),
    ];
    // prompt 0 is q0; the next navigable prompt is q1, so the app action and a0
    // are carried but q1 is not.
    expect(forkContextDigest(log, { kind: "up_to_message", prompt: 0 })).toBe(
      "User: q0\n\nAssistant: a0",
    );
  });
});
