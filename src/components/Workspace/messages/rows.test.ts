import { describe, expect, it } from "vitest";
import type { ViewItem } from "./pair";
import { buildRows, type Row, type RowCache } from "./rows";
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

describe("buildRows", () => {
  it("gathers consecutive steps into one run, broken by prose", () => {
    const items = [user(), think(), read("/a.ts"), read("/b.ts"), say(), edit("/a.ts"), say()];
    expect(shape(buildRows(items, new Map()).rows)).toEqual([0, "steps×3", 4, "steps×1", 6]);
  });

  it("keeps questions for the user and sub-agent launches out of runs", () => {
    const ask = tool("AskUserQuestion", { questions: [] }, null);
    const sub = tool("Agent", { description: "explore", prompt: "go" });
    const items = [user(), read("/a"), sub, read("/b"), ask];
    expect(shape(buildRows(items, new Map()).rows)).toEqual([0, "steps×1", 2, "steps×1", 4]);
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
    const { rows, quietFooters } = buildRows(items, new Map());
    expect(shape(rows)).toEqual([0, { fold: [1, "steps×1", 3, "steps×1"] }, 5, 6, "steps×1", 8]);
    const fold = rows[1].kind === "fold" ? rows[1].fold : null;
    expect(fold?.runSec).toBe(252);
    // The fold says how long turn one ran, so its footer (at its last item)
    // leaves the duration out.
    expect([...quietFooters]).toEqual([5]);
  });

  it("never folds the latest turn", () => {
    const items = [user(), say("a"), read("/a"), say("b"), read("/b"), say("c")];
    expect(shape(buildRows(items, new Map()).rows)).toEqual([0, 1, "steps×1", 3, "steps×1", 5]);
  });

  it("leaves a turn whose work is a single row as it is", () => {
    const items = [user(), read("/a"), read("/b"), say("done"), user(), say("hi")];
    expect(shape(buildRows(items, new Map()).rows)).toEqual([0, "steps×2", 3, 4, 5]);
  });

  it("folds everything after the prompt when the turn ends on a step", () => {
    const items = [user(), say("a"), read("/a"), say("b"), bash("ls"), user(), say("x")];
    expect(shape(buildRows(items, new Map()).rows)).toEqual([
      0,
      { fold: [1, "steps×1", 3, "steps×1"] },
      5,
      6,
    ]);
  });

  it("reuses unchanged runs and folds across passes", () => {
    const cache: RowCache = new Map();
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

  it("is thinking between calls, and says so at once on a thought", () => {
    expect(liveActivity([read("/a")])).toMatchObject({ label: "Thinking", idle: true });
    expect(liveActivity([read("/a"), think()])).toMatchObject({ label: "Thinking", idle: false });
  });
});
