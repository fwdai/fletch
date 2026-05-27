import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { claudeAdapter } from "./index";
import type { ChatItem, RawEvent } from "../types";

const here = fileURLToPath(new URL(".", import.meta.url));

function readJsonl(name: string): unknown[] {
  const raw = readFileSync(join(here, "fixtures", name), "utf8");
  return raw
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l));
}

function reduceAll(events: RawEvent[]): ChatItem[] {
  return events.reduce<ChatItem[]>(
    (acc, ev) => claudeAdapter.reduce(acc, ev),
    [],
  );
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
      { kind: "agent_message", text: "Hi there" },
      { kind: "notice", subtype: "slash_command", text: "/login" },
      { kind: "user_message", text: "what's next?" },
      { kind: "notice", subtype: "hook_output", text: "Hook stderr: x" },
      { kind: "agent_message", text: "All set." },
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
    const errorNotice = items.find(
      (it) => it.kind === "notice" && it.subtype === "error",
    );
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
