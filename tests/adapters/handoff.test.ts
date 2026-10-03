// Unit tests for the handoff transcript. These exercise the pure rendering in
// isolation — the forkAgent path is what guarantees the input log is the
// record-derived, policy-filtered history with only matched turns overlaid (so
// pending turns and hidden items never reach here); this file pins the
// serialization, the cut at the anchor turn, the start at a compaction, and
// the size budget.

import { describe, expect, it } from "vitest";
import {
  HANDOFF_INPUT_MAX,
  HANDOFF_TOOL_TEXT_MAX,
  handoffTranscript,
  handoffTranscriptBefore,
  OMITTED_NOTE,
  serializeHandoffItem,
} from "@/adapters/handoff";
import type { ChatItem } from "@/adapters/types";
import { APP_ACTION_PREFIX } from "@/delegation";

const user = (text: string, turnId?: string): ChatItem => ({
  kind: "user_message",
  text,
  ...(turnId ? { turnId } : {}),
});
const agent = (text: string): ChatItem => ({ kind: "agent_message", text });
const compaction = (summary: string): ChatItem => ({
  kind: "notice",
  subtype: "compact_summary",
  text: "Conversation compacted",
  summary,
});

describe("serializeHandoffItem", () => {
  it("renders user and agent messages", () => {
    expect(serializeHandoffItem(user("hello"))).toBe("User: hello");
    expect(serializeHandoffItem(agent("hi there"))).toBe("Assistant: hi there");
  });

  it("drops app-action (git delegation) user turns", () => {
    expect(serializeHandoffItem(user(`${APP_ACTION_PREFIX}open_pr`))).toBeNull();
  });

  it("drops empty agent messages", () => {
    expect(serializeHandoffItem(agent(""))).toBeNull();
  });

  it("serializes a tool call with its input", () => {
    const line = serializeHandoffItem({
      kind: "tool_call",
      id: "t1",
      name: "Bash",
      input: { command: "ls -la" },
    });
    expect(line).toContain("Assistant used tool `Bash`");
    expect(line).toContain('"command": "ls -la"');
  });

  it("flattens a subagent's nested conversation under the call", () => {
    const line = serializeHandoffItem({
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
    expect(
      serializeHandoffItem({ kind: "tool_result", tool_use_id: "t1", content: "42 files" }),
    ).toBe("Tool result:\n42 files");
    expect(
      serializeHandoffItem({
        kind: "tool_result",
        tool_use_id: "t1",
        content: "boom",
        is_error: true,
      }),
    ).toBe("Tool error:\nboom");
  });

  it("flattens anthropic content-block arrays in tool results", () => {
    const line = serializeHandoffItem({
      kind: "tool_result",
      tool_use_id: "t1",
      content: [{ type: "text", text: "line one" }],
    });
    expect(line).toBe("Tool result:\nline one");
  });

  it("renders non-text result blocks as a placeholder, not their JSON", () => {
    const line = serializeHandoffItem({
      kind: "tool_result",
      tool_use_id: "t1",
      content: [
        { type: "text", text: "screenshot taken" },
        { type: "image", source: { type: "base64", media_type: "image/png", data: "iVBORw0K" } },
      ],
    });
    expect(line).toBe("Tool result:\nscreenshot taken\n[image]");
  });

  it("caps a long tool result, noting how much was dropped", () => {
    const line = serializeHandoffItem({
      kind: "tool_result",
      tool_use_id: "t1",
      content: "x".repeat(HANDOFF_TOOL_TEXT_MAX + 12_345),
    });
    expect(line).toBe(`Tool result:\n${"x".repeat(HANDOFF_TOOL_TEXT_MAX)}\n[… 12,345 more chars]`);
  });

  it("leaves a result at exactly the cap untouched", () => {
    const content = "x".repeat(HANDOFF_TOOL_TEXT_MAX);
    expect(serializeHandoffItem({ kind: "tool_result", tool_use_id: "t1", content })).toBe(
      `Tool result:\n${content}`,
    );
  });

  it("caps a long tool input", () => {
    const line = serializeHandoffItem({
      kind: "tool_call",
      id: "t1",
      name: "Write",
      input: "y".repeat(HANDOFF_TOOL_TEXT_MAX + 500),
    });
    expect(line).toBe(
      `Assistant used tool \`Write\`:\n${"y".repeat(HANDOFF_TOOL_TEXT_MAX)}\n[… 500 more chars]`,
    );
  });

  it("never splits a surrogate pair at the cap", () => {
    // An emoji straddles the cut: its high surrogate is the last unit kept.
    const content = `${"x".repeat(HANDOFF_TOOL_TEXT_MAX - 1)}😀${"x".repeat(10)}`;
    const line = serializeHandoffItem({ kind: "tool_result", tool_use_id: "t1", content });
    expect(line).toBe(`Tool result:\n${"x".repeat(HANDOFF_TOOL_TEXT_MAX - 1)}\n[… 12 more chars]`);
  });

  it("applies the same caps to a subagent's nested items", () => {
    const line = serializeHandoffItem({
      kind: "tool_call",
      id: "t1",
      name: "Agent",
      input: {},
      children: [
        { kind: "tool_call", id: "t2", name: "Bash", input: "z".repeat(HANDOFF_TOOL_TEXT_MAX + 1) },
        {
          kind: "tool_result",
          tool_use_id: "t2",
          content: [
            { type: "text", text: "w".repeat(HANDOFF_TOOL_TEXT_MAX + 2) },
            { type: "image", source: { type: "base64", media_type: "image/png", data: "AAAA" } },
          ],
        },
      ],
    });
    expect(line).toContain(`${"z".repeat(HANDOFF_TOOL_TEXT_MAX)}\n[… 1 more chars]`);
    // The text block plus "\n[image]" overflow the cap by 10 chars.
    expect(line).toContain(`Tool result:\n${"w".repeat(HANDOFF_TOOL_TEXT_MAX)}\n[… 10 more chars]`);
    expect(line).not.toContain("AAAA");
  });

  it("labels reasoning and error notices, passes others through", () => {
    expect(
      serializeHandoffItem({ kind: "notice", subtype: "reasoning", text: "let me think" }),
    ).toBe("Assistant (thinking): let me think");
    expect(serializeHandoffItem({ kind: "notice", subtype: "error", text: "it failed" })).toBe(
      "Error: it failed",
    );
    expect(serializeHandoffItem({ kind: "notice", subtype: "info", text: "fyi" })).toBe("fyi");
    expect(serializeHandoffItem({ kind: "notice", subtype: "turn_end", text: "" })).toBeNull();
  });

  it("renders a compaction as the summary it left, not its marker", () => {
    expect(serializeHandoffItem(compaction("Summary: the plan"))).toBe("Summary: the plan");
    expect(
      serializeHandoffItem({ kind: "notice", subtype: "compact_summary", text: "Compacted" }),
    ).toBeNull();
  });

  it("never carries optimistic store-only queued messages", () => {
    expect(serializeHandoffItem({ kind: "queued_message", text: "later" })).toBeNull();
  });
});

describe("handoffTranscript", () => {
  it("returns null when the carried range has no prose", () => {
    expect(handoffTranscript([], null)).toBeNull();
    expect(handoffTranscript([user(`${APP_ACTION_PREFIX}x`)], null)).toBeNull();
  });

  it("joins the whole conversation through the end, tool context included", () => {
    const log: ChatItem[] = [
      user("run the tests"),
      { kind: "tool_call", id: "t1", name: "Bash", input: { command: "npm test" } },
      { kind: "tool_result", tool_use_id: "t1", content: "3 failing", is_error: true },
      agent("two are flaky"),
    ];
    expect(handoffTranscript(log, null)).toBe(
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
    expect(handoffTranscript(conversation, "t0")).toBe("User: q0\n\nAssistant: a0");
    expect(handoffTranscript(conversation, "t1")).toBe(
      "User: q0\n\nAssistant: a0\n\nUser: q1\n\nAssistant: a1",
    );
  });

  it("carries the whole conversation through the last turn", () => {
    expect(handoffTranscript(conversation, "t2")).toBe(
      "User: q0\n\nAssistant: a0\n\nUser: q1\n\nAssistant: a1\n\nUser: q2",
    );
  });

  it("stops at the next turn of any kind, app actions included", () => {
    // App-action turns are ordinary turns to the backend cut, so the transcript
    // must stop there too or it would brief the child on what it doesn't show.
    const log: ChatItem[] = [
      user("q0", "t0"),
      agent("a0"),
      user(`${APP_ACTION_PREFIX}open_pr`, "t-pr"),
      agent("pr opened"),
      user("q1", "t1"),
    ];
    expect(handoffTranscript(log, "t0")).toBe("User: q0\n\nAssistant: a0");
    expect(handoffTranscript(log, "t-pr")).toBe(
      "User: q0\n\nAssistant: a0\n\nAssistant: pr opened",
    );
  });

  it("does not let a user message without a turn row bound the cut", () => {
    // The backend cuts at the next *matched* turn; a message with no turn row
    // (typed into the native view) sits inside the carried range.
    const log: ChatItem[] = [user("q0", "t0"), agent("a0"), user("typed"), agent("a1")];
    expect(handoffTranscript(log, "t0")).toBe(
      "User: q0\n\nAssistant: a0\n\nUser: typed\n\nAssistant: a1",
    );
  });

  it("carries nothing when the anchor isn't in the log", () => {
    expect(handoffTranscript(conversation, "gone")).toBeNull();
  });

  describe("after a compaction", () => {
    // Compacted mid-turn twice, as an automatic compaction does.
    const log: ChatItem[] = [
      user("q0", "t0"),
      agent("a0"),
      user("q1", "t1"),
      compaction("Summary: q0, and q1 so far"),
      agent("a1"),
      user("q2", "t2"),
      compaction("Summary: q0 to q2 so far"),
      agent("a2"),
    ];

    it("starts at the last compaction's summary, dropping what it replaced", () => {
      expect(handoffTranscript(log, null)).toBe("Summary: q0 to q2 so far\n\nAssistant: a2");
    });

    it("only counts a compaction before the cut", () => {
      expect(handoffTranscript(log, "t1")).toBe("Summary: q0, and q1 so far\n\nAssistant: a1");
      expect(handoffTranscript(log, "t0")).toBe("User: q0\n\nAssistant: a0");
    });
  });

  describe("past the size budget", () => {
    // Ten turns of 100k chars each: 1M chars, well past the 400k budget.
    const big = (i: number) => `${i}`.padEnd(100_000, ".");
    const log: ChatItem[] = Array.from({ length: 10 }, (_, i) => user(big(i), `t${i}`));

    it("keeps the most recent whole items behind a note", () => {
      const transcript = handoffTranscript(log, null) ?? "";
      expect(transcript.length).toBeLessThanOrEqual(HANDOFF_INPUT_MAX);
      expect(transcript.startsWith(`${OMITTED_NOTE}\n\nUser: 7`)).toBe(true);
      expect(transcript.endsWith(`User: ${big(9)}`)).toBe(true);
      expect(transcript).not.toContain("User: 6");
    });

    it("keeps the end of a single item larger than the whole budget", () => {
      const huge = `start${"x".repeat(HANDOFF_INPUT_MAX)}end`;
      const transcript = handoffTranscript([user(huge)], null) ?? "";
      expect(transcript.length).toBeLessThanOrEqual(HANDOFF_INPUT_MAX);
      expect(transcript.startsWith(`${OMITTED_NOTE}\n\nxxx`)).toBe(true);
      expect(transcript.endsWith("end")).toBe(true);
    });
  });
});

describe("handoffTranscriptBefore", () => {
  const conversation: ChatItem[] = [
    user("q0", "t0"),
    agent("a0"),
    user("q1", "t1"),
    agent("a1"),
    user("q2", "t2"),
  ];

  it("keeps everything up to the turn's prompt, and none of it", () => {
    expect(handoffTranscriptBefore(conversation, "t1")).toBe("User: q0\n\nAssistant: a0");
    expect(handoffTranscriptBefore(conversation, "t2")).toBe(
      "User: q0\n\nAssistant: a0\n\nUser: q1\n\nAssistant: a1",
    );
  });

  it("carries nothing before the first turn, or for a turn not in the log", () => {
    expect(handoffTranscriptBefore(conversation, "t0")).toBeNull();
    expect(handoffTranscriptBefore(conversation, "gone")).toBeNull();
  });

  it("starts at the last compaction before the turn", () => {
    const log: ChatItem[] = [
      user("q0", "t0"),
      compaction("Summary: q0"),
      agent("a0"),
      user("q1", "t1"),
    ];
    expect(handoffTranscriptBefore(log, "t1")).toBe("Summary: q0\n\nAssistant: a0");
  });
});
