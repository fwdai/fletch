import { describe, expect, it } from "vitest";
import { claudeAdapter } from "@/adapters/claude/index";
import type { ChatItem } from "@/store";
import { type ViewItem, visibleItems } from "./pair";
import { buildRows, newRowCache, type Row } from "./rows";
import { liveActivity, summarizeSteps, summaryParts } from "./steps";

let seq = 0;
const user = (text = "do it", timing?: { startedAt: number; endedAt?: number }): ViewItem => ({
  kind: "user_message",
  text,
  ...timing,
});
const say = (text = "ok"): ViewItem => ({ kind: "agent_message", text });
const think = (text = "hmm"): ViewItem => ({ kind: "notice", subtype: "reasoning", text });
const tool = (
  name: string,
  input: unknown,
  result: unknown = "done",
  isError = false,
): ViewItem => {
  const id = `t${seq++}`;
  return {
    kind: "tool_pair",
    call: { kind: "tool_call", id, name, input },
    result:
      result === null
        ? null
        : { kind: "tool_result", tool_use_id: id, content: result, is_error: isError },
  };
};
const read = (path: string) => tool("Read", { file_path: path });
const edit = (path: string, oldS = "a", newS = "b\nc") =>
  tool("Edit", { file_path: path, old_string: oldS, new_string: newS });
const bash = (command: string, description?: string) => tool("Bash", { command, description });

/** Rows as a compact shape: item indices, run lengths, folds with their rows. */
function shape(rows: Row[]): unknown[] {
  return rows.map((r) => {
    if (r.kind === "item") return r.index;
    if (r.kind === "steps") return `steps×${r.run.items.length}`;
    return { fold: shape(r.fold.rows) };
  });
}

/** Every item the rows hold, in order, folds and runs opened. */
function flatten(rows: Row[], items: ViewItem[]): ViewItem[] {
  return rows.flatMap((r) => {
    if (r.kind === "item") return [items[r.index]];
    if (r.kind === "steps") return r.run.items;
    return flatten(r.fold.rows, items);
  });
}

describe("buildRows", () => {
  it("gathers consecutive steps into one run, broken by prose", () => {
    const items = [user(), think(), read("/a.ts"), read("/b.ts"), say(), edit("/a.ts"), say()];
    expect(shape(buildRows(items, newRowCache()).rows)).toEqual([0, "steps×3", 4, "steps×1", 6]);
  });

  // The rows only regroup the items; nothing is left out. Thoughts in
  // particular: live, mid-run, between a call and its result, and in a turn
  // folded away — each is in the rows exactly once, in its place.
  it("holds every item exactly once, thoughts included", () => {
    const items = [
      user("one", { startedAt: 0, endedAt: 1000 }),
      think("first look"),
      read("/a"),
      think("now change it"),
      edit("/a"),
      say("done"),
      user("two"),
      think("checking"),
      bash("npm test"),
      think("passes"),
      say("fine"),
      think("one more thing"),
      tool("Read", { file_path: "/b" }, null),
    ];
    const { rows } = buildRows(items, newRowCache());
    expect(flatten(rows, items)).toEqual(items);
  });

  it("keeps questions for the user and sub-agent launches out of runs", () => {
    const ask = tool("AskUserQuestion", { questions: [] }, null);
    const sub = tool("Agent", { description: "explore", prompt: "go" });
    const items = [user(), read("/a"), sub, read("/b"), ask];
    expect(shape(buildRows(items, newRowCache()).rows)).toEqual([0, "steps×1", 2, "steps×1", 4]);
  });

  it("folds an older turn's work and keeps its prompt and answer", () => {
    const items = [
      user("one", { startedAt: 0, endedAt: 252_000 }),
      say("looking"),
      read("/a"),
      say("now editing"),
      edit("/a"),
      say("done"),
      user("two"),
      read("/b"),
      say("fine"),
    ];
    const { rows, quietFooters } = buildRows(items, newRowCache());
    expect(shape(rows)).toEqual([0, { fold: [1, "steps×1", 3, "steps×1"] }, 5, 6, "steps×1", 8]);
    const fold = rows[1].kind === "fold" ? rows[1].fold : null;
    expect(fold?.runSec).toBe(252);
    // The fold says how long turn one ran, so its footer (at its last item)
    // leaves the duration out.
    expect([...quietFooters]).toEqual([5]);
  });

  it("never folds the latest turn", () => {
    const items = [user(), say("a"), read("/a"), say("b"), read("/b"), say("c")];
    expect(shape(buildRows(items, newRowCache()).rows)).toEqual([0, 1, "steps×1", 3, "steps×1", 5]);
  });

  it("leaves a turn whose work is a single row as it is", () => {
    const items = [user(), read("/a"), read("/b"), say("done"), user(), say("hi")];
    expect(shape(buildRows(items, newRowCache()).rows)).toEqual([0, "steps×2", 3, 4, 5]);
  });

  it("folds everything after the prompt when the turn ends on a step", () => {
    const items = [user(), say("a"), read("/a"), say("b"), bash("ls"), user(), say("x")];
    expect(shape(buildRows(items, newRowCache()).rows)).toEqual([
      0,
      { fold: [1, "steps×1", 3, "steps×1"] },
      5,
      6,
    ]);
  });

  // A run is named by its first tool call and a fold by its prompt, so a page
  // of older history prepended at the top (the phone's "Load older messages")
  // moves every index but no key: an open run stays open, and the run that
  // now sits at its old index doesn't inherit its state.
  it("keeps run and fold keys through a prepend", () => {
    const older = [
      { ...user("zero", { startedAt: 0, endedAt: 1 }), turnId: "t0" },
      read("/z"),
      say("done"),
    ];
    const items = [
      { ...user("one", { startedAt: 0, endedAt: 1 }), turnId: "t1" },
      read("/a"),
      say("looking"),
      edit("/a"),
      say("ok"),
      user("two"),
      read("/b"),
      think(),
    ];
    const keys = (rows: Row[]) =>
      rows.flatMap((r) => (r.kind === "item" ? [] : [r.kind === "fold" ? r.fold.key : r.run.key]));
    const before = keys(buildRows(items, newRowCache()).rows);
    const after = keys(buildRows([...older, ...items], newRowCache()).rows);
    expect(after.slice(-before.length)).toEqual(before);
    expect(before[0]).toBe("f:u:t1");
    expect(before[1]).toBe(`s:tp:${(items[6] as { call: { id: string } }).call.id}`);
  });

  it("reuses unchanged runs and folds across passes", () => {
    const cache = newRowCache();
    const items = [
      user(),
      say("a"),
      read("/a"),
      say("b"),
      read("/b"),
      say("c"),
      user(),
      read("/c"),
    ];
    const first = buildRows(items, cache).rows;
    const grown = [...items, read("/d")];
    const second = buildRows(grown, cache).rows;
    expect(second[1]).toEqual(first[1]);
    if (first[1].kind === "fold" && second[1].kind === "fold") {
      expect(second[1].fold).toBe(first[1].fold);
    }
    // The live run grew, so it's a new object.
    const lastRun = (rows: Row[]) => rows[rows.length - 1];
    expect(lastRun(second)).not.toBe(lastRun(first));
  });
});

describe("from Claude's records to rows", () => {
  const assistant = (block: Record<string, unknown>) => ({
    type: "assistant",
    thinkingDisplay: "updates",
    message: { id: "m1", role: "assistant", model: "claude-opus-5-5", content: [block] },
  });
  const toolResult = (id: string) => ({
    type: "user",
    message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content: "x" }] },
  });
  const thoughts = (xs: ViewItem[]) =>
    xs.flatMap((it) => (it.kind === "notice" && it.subtype === "reasoning" ? [it.text] : []));

  // The records as Claude Code writes them: one content block per line, the
  // reasoning itself redacted to "" plus a signature, and the short readable
  // updates it shares as blocks of their own. The old inline view showed each
  // readable one as a THINKING block; here each is a Thought step inside its
  // run, and a redacted one leaves nothing — in the live log and in the
  // rebuild from these records alike.
  it("gives every readable thought a step and a redacted one none", () => {
    const lines = [
      { type: "user", message: { role: "user", content: "go" } },
      assistant({ type: "thinking", thinking: "", signature: "r" }),
      assistant({ type: "thinking", thinking: "Reading the reducer first.", signature: "s" }),
      assistant({ type: "tool_use", id: "t1", name: "Read", input: { file_path: "/a" } }),
      toolResult("t1"),
      assistant({ type: "thinking", thinking: "", signature: "r" }),
      assistant({
        type: "tool_use",
        id: "t2",
        name: "Edit",
        input: { file_path: "/a", old_string: "a", new_string: "b" },
      }),
      toolResult("t2"),
      assistant({ type: "thinking", thinking: "Now the test.", signature: "s" }),
      assistant({ type: "text", text: "Done." }),
    ];
    const log = claudeAdapter
      .normalizeTranscript(lines)
      .reduce<ChatItem[]>((acc, ev) => claudeAdapter.reduce(acc, ev), []);
    const items = visibleItems(log, claudeAdapter.policy);
    expect(thoughts(items)).toEqual(["Reading the reducer first.", "Now the test."]);

    const { rows } = buildRows(items, newRowCache());
    expect(shape(rows)).toEqual([0, "steps×4", 5]);
    expect(thoughts(flatten(rows, items))).toEqual(thoughts(items));
  });
});

describe("summaryParts", () => {
  const texts = (items: ViewItem[]) =>
    summaryParts(summarizeSteps(items)).map((p) => (p.code ? `${p.text} ${p.code}` : p.text));

  it("names one or two files and counts beyond", () => {
    expect(texts([edit("/x/a.ts"), edit("/x/a.ts"), edit("/x/b.ts")])).toEqual([
      "changed a.ts, b.ts",
    ]);
    expect(texts([edit("/a"), edit("/b"), edit("/c")])).toEqual(["changed 3 files"]);
  });

  it("orders by consequence and puts failures last", () => {
    const items = [
      read("/r.ts"),
      bash("cargo test"),
      edit("/e.ts"),
      tool("Grep", { pattern: "x" }, "", true),
    ];
    expect(texts(items)).toEqual([
      "changed e.ts",
      "ran cargo test",
      "read r.ts",
      "searched for x",
      "1 failed",
    ]);
  });

  it("names a lone command without its cd preamble", () => {
    expect(texts([bash("cd /repo && npm test")])).toEqual(["ran npm test"]);
  });

  it("totals line changes across a run", () => {
    const s = summarizeSteps([edit("/a", "1", "2\n3"), edit("/b", "x\ny", "z")]);
    expect(s.changed).toMatchObject({ additions: 3, deletions: 3 });
  });
});

describe("summaryParts fallbacks", () => {
  const texts = (items: ViewItem[], taskFor?: Parameters<typeof summarizeSteps>[1]) =>
    summaryParts(summarizeSteps(items, taskFor)).map((p) =>
      p.code ? `${p.text} ${p.code}` : p.text,
    );

  it("never leaves a run unlabelled", () => {
    const orphan: ViewItem = { kind: "tool_result", tool_use_id: "gone", content: "x" };
    const hook: ViewItem = { kind: "notice", subtype: "hook_output", text: "lint ok" };
    expect(texts([orphan])).toEqual(["received 1 result"]);
    expect(texts([hook, hook])).toEqual(["ran 2 hooks"]);
    expect(texts([{ kind: "notice", subtype: "info", text: "hi" }])).toEqual(["1 step"]);
  });

  // A background command fails after its launch result landed; the result in
  // the log says nothing about it, the task does.
  it("counts a failed background task as a failure of the run", () => {
    const launch = bash("npm run dev &");
    const id = launch.kind === "tool_pair" ? launch.call.id : "";
    const failed = (toolUseId: string) =>
      toolUseId === id
        ? ({ status: "failed", failureStatus: "exited 1" } as ReturnType<
            NonNullable<Parameters<typeof summarizeSteps>[1]>
          >)
        : undefined;
    expect(texts([launch])).toEqual(["ran npm run dev &"]);
    expect(texts([launch], failed)).toEqual(["ran npm run dev &", "1 failed"]);
  });
});

describe("liveActivity", () => {
  it("says what the call in flight is doing", () => {
    expect(
      liveActivity([read("/a.ts"), tool("Read", { file_path: "/src/b.tsx" }, null)]).label,
    ).toBe("Reading b.tsx");
  });

  it("prefers the agent's own description of a command", () => {
    const pending = tool(
      "Bash",
      { command: "npx vitest run", description: "Run the unit tests" },
      null,
    );
    expect(liveActivity([pending]).label).toBe("Running the unit tests");
  });

  it("counts parallel calls of one kind", () => {
    const items = [
      tool("Read", { file_path: "/a" }, null),
      tool("Read", { file_path: "/b" }, null),
    ];
    expect(liveActivity(items).label).toBe("Reading 2 files");
  });

  it("says it's thinking only with a thought to show for it", () => {
    expect(liveActivity([read("/a")])).toMatchObject({ label: "Working", idle: true });
    expect(liveActivity([read("/a"), think()])).toMatchObject({ label: "Thinking", idle: false });
  });
});
