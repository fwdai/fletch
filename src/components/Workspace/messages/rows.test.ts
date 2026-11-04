import { describe, expect, it } from "vitest";
import { claudeAdapter } from "@/adapters/claude/index";
import { APP_ACTION_PREFIX } from "@/delegation";
import type { ChatItem } from "@/store";
import { type ViewItem, visibleItems } from "./pair";
import { buildRows, footerFor, newRowCache, type Row, turnFooters, turnSpans } from "./rows";
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

/** Rows as a compact shape: item indices and run lengths. */
function shape(rows: Row[]): unknown[] {
  return rows.map((r) => (r.kind === "item" ? r.index : `steps×${r.run.items.length}`));
}

/** Every item the rows hold, in order, runs opened. */
function flatten(rows: Row[], items: ViewItem[]): ViewItem[] {
  return rows.flatMap((r) => (r.kind === "item" ? [items[r.index]] : r.run.items));
}

describe("buildRows", () => {
  it("gathers consecutive steps into one run, broken by prose", () => {
    const items = [user(), think(), read("/a.ts"), read("/b.ts"), say(), edit("/a.ts"), say()];
    expect(shape(buildRows(items, newRowCache()).rows)).toEqual([0, "steps×3", 4, "steps×1", 6]);
  });

  // The rows only regroup the items; nothing is left out. Thoughts in
  // particular: live, mid-run, between a call and its result, and in a settled
  // turn — each is in the rows exactly once, in its place.
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

  // Only steps collapse, so a settled turn reads as the latest one does: what
  // the agent said, each run of work under the words that introduced it.
  it("keeps what the agent says on the page in every turn", () => {
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
    expect(shape(buildRows(items, newRowCache()).rows)).toEqual([
      0,
      1,
      "steps×1",
      3,
      "steps×1",
      5,
      6,
      "steps×1",
      8,
    ]);
  });

  // The answer needn't be the last thing in a turn: here a scheduled wake-up
  // fired after it, called a tool to end its loop, and added one line.
  it("keeps an answer a later tool call follows on the page", () => {
    const answer = say("the assessment");
    const items = [
      user("audit this"),
      read("/a"),
      answer,
      tool("ScheduleWakeup", { stop: true }),
      say("nothing further"),
      user("what are you doing?"),
      say("nothing"),
    ];
    const { rows } = buildRows(items, newRowCache());
    expect(shape(rows)).toEqual([0, "steps×1", 2, "steps×1", 4, 5, 6]);
    expect(rows.some((r) => r.kind === "item" && items[r.index] === answer)).toBe(true);
  });

  it("keeps sub-agents on the page in a settled turn", () => {
    const sub = tool("Agent", { description: "explore", prompt: "go" });
    const items = [user(), read("/a"), sub, read("/b"), say("done"), user(), say("hi")];
    expect(shape(buildRows(items, newRowCache()).rows)).toEqual([
      0,
      "steps×1",
      2,
      "steps×1",
      4,
      5,
      6,
    ]);
  });

  // A run is named by its first tool call, so a page of older history
  // prepended at the top (the phone's "Load older messages") moves every index
  // but no key: an open run stays open, and the run that now sits at its old
  // index doesn't inherit its state.
  it("keeps run keys through a prepend", () => {
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
    const keys = (rows: Row[]) => rows.flatMap((r) => (r.kind === "item" ? [] : [r.run.key]));
    const before = keys(buildRows(items, newRowCache()).rows);
    const after = keys(buildRows([...older, ...items], newRowCache()).rows);
    expect(after.slice(-before.length)).toEqual(before);
    expect(before[0]).toBe(`s:tp:${(items[1] as { call: { id: string } }).call.id}`);
  });

  it("reuses unchanged runs across passes", () => {
    const cache = newRowCache();
    const items = [user(), say("a"), read("/a"), say("b"), read("/b")];
    const first = buildRows(items, cache).rows;
    const second = buildRows([...items, read("/d")], cache).rows;
    // The settled run keeps its object, so its memoized row skips re-rendering.
    const run = (rows: Row[], at: number) => (rows[at].kind === "steps" ? rows[at].run : null);
    expect(run(second, 2)).toBe(run(first, 2));
    // The live run grew, so it's a new object.
    expect(run(second, 4)).not.toBe(run(first, 4));
  });
});

describe("turnFooters", () => {
  const timed = (startedAt: number, endedAt?: number, providerRunMs?: number) =>
    ({ ...user("go", { startedAt, endedAt }), providerRunMs }) as ViewItem;

  it("closes a turn at its last item, with its prose", () => {
    const items = [timed(0, 90_000), say("one"), read("/a"), say("two"), user("next"), say("hi")];
    const { footers } = turnFooters(items);
    expect(footers[3]).toMatchObject({ runSec: 90, copyText: "one\n\ntwo" });
    expect(footers.filter(Boolean)).toHaveLength(1);
  });

  // A git button's app action is a message to the agent but not a turn of the
  // user's, so the turn it was clicked in ends after its reply and ran for both.
  it("ends a turn after the app action clicked in it, counting its time", () => {
    const items = [
      { ...user("one", { startedAt: 0, endedAt: 60_000 }), turnId: "t1" },
      say("looking"),
      read("/a"),
      say("done"),
      {
        ...user(`${APP_ACTION_PREFIX}commit-pr`, { startedAt: 70_000, endedAt: 80_000 }),
        turnId: "t2",
      },
      bash("git commit"),
      say("committed"),
      bash("git push"),
      say("opened #1"),
      user("two"),
      say("hi"),
    ];
    expect(turnSpans(items)).toEqual([
      { head: 0, end: 9 },
      { head: 9, end: 11 },
    ]);
    const { footers } = turnFooters(items);
    expect(footers[8]).toMatchObject({ runSec: 70, forkTurnId: "t2" });
    expect(footers.slice(0, 8).every((f) => f === null)).toBe(true);
  });

  // Claude's `result` closes the turn; what it runs on its own afterwards
  // (answering a background task, waking on a schedule) is the same work. The
  // idle time between those runs is not: it's in neither number.
  it("counts the runs the provider opened on its own", () => {
    const { footers } = turnFooters([timed(0, 141_000, 180_000), say("done")]);
    expect(footers[1]?.runSec).toBe(321);
  });

  it("leaves an open turn without a footer, naming when it began", () => {
    const { footers, openStartedAt } = turnFooters([timed(5000), say("working")]);
    expect(footers).toEqual([null, null]);
    expect(openStartedAt).toBe(5000);
  });

  it("keeps a turn open while an app action in it runs", () => {
    const items = [
      timed(0, 60_000),
      say("done"),
      user(`${APP_ACTION_PREFIX}commit`, { startedAt: 70_000 }),
      bash("git commit"),
    ];
    const { footers, openStartedAt } = turnFooters(items);
    expect(footers.every((f) => f === null)).toBe(true);
    expect(openStartedAt).toBe(70_000);
  });

  it("has nothing to say about a turn with no timing", () => {
    expect(turnFooters([user(), say("a")]).footers).toEqual([null, null]);
  });

  // Between Claude's `result` and its own next run the prompt has ended, so the
  // footer exists; while that run is live it isn't the turn's last word yet.
  it("holds the footer back while the agent is on that turn", () => {
    const items = [timed(0, 60_000), say("a"), timed(70_000, 80_000), say("b")];
    const { footers } = turnFooters(items);
    const { rows } = buildRows(items, newRowCache());
    const closing = rows[rows.length - 1];
    expect(footerFor(closing, footers, false, 2)).not.toBeNull();
    expect(footerFor(closing, footers, true, 2)).toBeNull();
    // An earlier turn's footer stands whatever the latest is doing.
    expect(footerFor(rows[1], footers, true, 2)).not.toBeNull();
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

describe("a turn a scheduled wake-up added to", () => {
  const assistant = (block: Record<string, unknown>) => ({
    type: "assistant",
    message: { id: "m1", role: "assistant", model: "claude-opus-5-5", content: [block] },
  });

  // The records as Claude Code wrote them for the session that found this: the
  // wake-up's prompt is injected (`isMeta`), so it opens no turn of the user's.
  it("leaves the answer it followed on the page", () => {
    const lines = [
      { type: "user", message: { role: "user", content: "audit the app" } },
      assistant({ type: "text", text: "The full assessment." }),
      { type: "user", isMeta: true, message: { role: "user", content: "check the surveys" } },
      assistant({ type: "tool_use", id: "w1", name: "ScheduleWakeup", input: { stop: true } }),
      {
        type: "user",
        message: {
          role: "user",
          content: [{ type: "tool_result", tool_use_id: "w1", content: "stopped" }],
        },
      },
      assistant({ type: "text", text: "Nothing further is pending." }),
    ];
    const log = claudeAdapter
      .normalizeTranscript(lines)
      .reduce<ChatItem[]>((acc, ev) => claudeAdapter.reduce(acc, ev), []);
    const items = visibleItems(log, claudeAdapter.policy);
    const { rows } = buildRows(items, newRowCache());
    expect(shape(rows)).toEqual([0, 1, "steps×1", 3]);
    expect(items[1]).toMatchObject({ kind: "agent_message", text: "The full assessment." });
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
