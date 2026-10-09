import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { reduce as claudeReduce } from "@/adapters/claude/reduce";
import { getAdapter } from "@/adapters/index";
import type { ChatItem, RawEvent } from "@/adapters/types";

const adaptersDir = join(fileURLToPath(new URL(".", import.meta.url)), "..");

function readJsonl(path: string): RawEvent[] {
  return readFileSync(join(adaptersDir, path), "utf8")
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l) as RawEvent);
}

/** Every item in the log, sub-agent children included. */
function flatten(items: ChatItem[]): ChatItem[] {
  return items.flatMap((it) =>
    it.kind === "tool_call" && it.children ? [it, ...flatten(it.children)] : [it],
  );
}

// Live stream captures per provider (antigravity has no live stream).
const LIVE: Array<[provider: string, fixture: string]> = [
  ["claude", "claude/fixtures/live-events.jsonl"],
  ["codex", "codex/fixtures/sample.jsonl"],
  ["cursor", "cursor/fixtures/sample.jsonl"],
  ["cursor", "cursor/fixtures/reasoning.jsonl"],
  ["opencode", "opencode/fixtures/sample.jsonl"],
  ["pi", "pi/fixtures/sample.jsonl"],
];

describe("recordSeq on live-stream items", () => {
  it.each(LIVE)("%s (%s) leaves recordSeq unset", (provider, fixture) => {
    const adapter = getAdapter(provider);
    const items = readJsonl(fixture).reduce<ChatItem[]>((acc, ev) => adapter.reduce(acc, ev), []);
    expect(items.length).toBeGreaterThan(0);
    for (const item of flatten(items)) expect(item).not.toHaveProperty("recordSeq");
  });

  it("normalizing bare bodies (no seqs) stamps no event", () => {
    const events = getAdapter("codex").normalizeTranscript(
      readJsonl("codex/fixtures/rollout.jsonl"),
    );
    expect(events.length).toBeGreaterThan(0);
    for (const ev of events) expect(ev).not.toHaveProperty("recordSeq");
  });
});

describe("recordSeq across records", () => {
  const delta = (text: string, recordSeq: number): RawEvent => ({
    type: "stream_event",
    event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    recordSeq,
  });

  it("keeps the seq of the record that opened a message later records extend", () => {
    const items = [delta("Hel", 1), delta("lo", 2)].reduce<ChatItem[]>(claudeReduce, []);
    expect(items).toEqual([
      { kind: "agent_message", text: "Hello", streaming: true, recordSeq: 1 },
    ]);
  });

  it("keeps the seq of the record that opened a tool call a later record settles", () => {
    const toolUse = (recordSeq: number): RawEvent => ({
      type: "assistant",
      message: { content: [{ type: "tool_use", id: "t1", name: "Read", input: {} }] },
      recordSeq,
    });
    const items = [toolUse(1), toolUse(2)].reduce<ChatItem[]>(claudeReduce, []);
    expect(items).toEqual([{ kind: "tool_call", id: "t1", name: "Read", input: {}, recordSeq: 1 }]);
  });

  it("leaves items a record didn't create referentially intact", () => {
    const first = claudeReduce([], { type: "user", message: { content: "hi" }, recordSeq: 1 });
    const next = claudeReduce(first, { type: "result", subtype: "success", recordSeq: 2 });
    expect(next[0]).toBe(first[0]);
    expect(next).toEqual([
      { kind: "user_message", text: "hi", recordSeq: 1 },
      { kind: "notice", subtype: "turn_end", text: "success", recordSeq: 2 },
    ]);
  });
});
