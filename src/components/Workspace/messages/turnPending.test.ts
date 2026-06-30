import { describe, expect, it } from "vitest";
import type { ViewItem } from "./pair";
import { isTurnPending } from "./turnPending";

describe("isTurnPending", () => {
  it("is true when the last row is a user message", () => {
    const items: ViewItem[] = [{ kind: "user_message", text: "hello" }];
    expect(isTurnPending(items)).toBe(true);
  });

  it("is true when the last row is a slash-command notice", () => {
    const items: ViewItem[] = [{ kind: "notice", subtype: "slash_command", text: "/compact" }];
    expect(isTurnPending(items)).toBe(true);
  });

  it("is false when agent output has landed", () => {
    const items: ViewItem[] = [
      { kind: "user_message", text: "hello" },
      { kind: "agent_message", text: "hi" },
    ];
    expect(isTurnPending(items)).toBe(false);
  });

  it("is false for a mid-turn queued follow-up", () => {
    const items: ViewItem[] = [
      { kind: "user_message", text: "hello" },
      { kind: "tool_pair", call: { id: "t1", name: "bash", input: "ls" }, result: null },
      { kind: "queued_message", text: "also do this" },
    ];
    expect(isTurnPending(items)).toBe(false);
  });

  it("is false on an empty log", () => {
    expect(isTurnPending([])).toBe(false);
  });
});
