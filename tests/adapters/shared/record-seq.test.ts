import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { reduce as claudeReduce } from "@/adapters/claude/reduce";
import { getAdapter } from "@/adapters/index";
import type { ChatItem, RawEvent } from "@/adapters/types";
import { codexPromptIndices, readRecordFixture } from "./prompt-records";

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

describe("prompt bubbles sit on the record the backend positions their turn on", () => {
  // A turn row overlays only the user_message stamped with its position, so
  // the two must agree or the prompt renders twice (bubble + row).
  const seqOf = (i: number) => 100 + i; // offset so a seq can't pass for an index
  function promptSeqs(provider: string, lines: Record<string, unknown>[]) {
    const adapter = getAdapter(provider);
    const items = adapter
      .normalizeTranscript(
        lines,
        lines.map((_, i) => seqOf(i)),
      )
      .reduce<ChatItem[]>((acc, ev) => adapter.reduce(acc, ev), []);
    return { items, prompts: items.filter((it) => it.kind === "user_message") };
  }

  it.each(["codex/fixtures/rollout.jsonl", "codex/fixtures/rollout-0153.jsonl"])(
    "codex (%s): the response_item twin, else the event",
    (fixture) => {
      const lines = readRecordFixture(fixture);
      const expected = codexPromptIndices(lines).map(seqOf);
      expect(expected.length).toBeGreaterThan(0);
      expect(promptSeqs("codex", lines).prompts.map((it) => it.recordSeq)).toEqual(expected);
    },
  );

  it("codex: an event whose twin is absent or differs sits on itself", () => {
    // 0.135: the user response_item before the event is AGENTS.md context.
    const lines = readRecordFixture("codex/fixtures/rollout.jsonl");
    const at = lines.findIndex((l) => (l.payload as { type?: string }).type === "user_message");
    expect(codexPromptIndices(lines)).toEqual([at]);
  });

  it("claude: the user record itself", () => {
    // `claude_prompt`: a top-level, unflagged `user` record that is no tool
    // result. A slash command is one too, but draws as a notice, not a bubble.
    const lines = readRecordFixture("claude/fixtures/transcript.jsonl");
    const isPrompt = (l: Record<string, unknown>) =>
      l.type === "user" &&
      !["isMeta", "isSynthetic", "isSidechain", "isCompactSummary"].some((f) => l[f] === true) &&
      !JSON.stringify(l).includes('"tool_result"');
    const isSlash = (l: Record<string, unknown>) => JSON.stringify(l).includes("<command-name>");
    const expected = lines.flatMap((l, i) => (isPrompt(l) && !isSlash(l) ? [seqOf(i)] : []));
    const { items, prompts } = promptSeqs("claude", lines);
    expect(expected.length).toBeGreaterThan(0);
    expect(prompts.map((it) => it.recordSeq)).toEqual(expected);
    const slash = lines.findIndex(isSlash);
    expect(items.find((it) => it.recordSeq === seqOf(slash))).toMatchObject({
      kind: "notice",
      subtype: "slash_command",
    });
  });
});
