import { describe, expect, it } from "vitest";
import type { ChatItem } from "@/adapters";
import type { SessionRecord, UserTurn } from "@/api";
import { mergeUserTurns, reduceRecords, turnsOnPage } from "@/helpers";
import { codexPromptIndices, readRecordFixture } from "../adapters/shared/prompt-records";

const OWN = "own";
const PARENT = "parent";

function turn(over: Partial<UserTurn>): UserTurn {
  return {
    turn_id: "t",
    session_id: OWN,
    seq: 0,
    text: "",
    attachments: [],
    native_id: null,
    started_at: null,
    ended_at: null,
    outcome: null,
    position: null,
    ...over,
  };
}

/** A turn paired with its prompt record at `seq`, run to its end. */
const echoed = (turnId: string, text: string, seq: number, session = OWN): UserTurn =>
  turn({
    turn_id: turnId,
    session_id: session,
    text,
    native_id: `n${seq}`,
    position: seq,
    started_at: 1,
    ended_at: 2,
    outcome: "completed",
  });

const user = (text: string, recordSeq: number, recordSession = OWN): ChatItem => ({
  kind: "user_message",
  text,
  recordSeq,
  recordSession,
});
const agent = (text: string, recordSeq: number, recordSession = OWN): ChatItem => ({
  kind: "agent_message",
  text,
  recordSeq,
  recordSession,
});

/** `[kind, text, turnId]` of each item, the shape most cases assert on. */
const shape = (items: ChatItem[]) =>
  items.map((it) => [
    it.kind,
    "text" in it ? it.text : "",
    it.kind === "user_message" || it.kind === "queued_message" ? it.turnId : undefined,
  ]);

describe("mergeUserTurns", () => {
  it("returns the log untouched with no turns", () => {
    const items = [user("hi", 1)];
    expect(mergeUserTurns(items, [])).toBe(items);
  });

  it("overlays a paired turn onto exactly the record it names", () => {
    // Two identical prompts: only the seq tells them apart.
    const items = [user("yes", 1), agent("ok", 2), user("yes", 3), agent("ok", 4)];
    const out = mergeUserTurns(items, [echoed("t2", "yes", 3)]);
    expect(out[0]).toEqual(user("yes", 1));
    expect(out[2]).toEqual({ ...user("yes", 3), turnId: "t2", startedAt: 1, endedAt: 2 });
    expect(out).toHaveLength(4);
  });

  it("restores the typed text and attachments, guarded by the prefix", () => {
    const padded = "look\nAttached file: /tmp/a.png";
    const t = { ...echoed("t1", "look", 1), attachments: ["/tmp/a.png"] };
    const [item] = mergeUserTurns([user(padded, 1)], [t]);
    expect(item).toMatchObject({ text: "look", attachments: ["/tmp/a.png"], turnId: "t1" });
    const [other] = mergeUserTurns([user("something else", 1)], [t]);
    expect(other).toMatchObject({ text: "something else", attachments: ["/tmp/a.png"] });
  });

  it("draws an echo-less stopped turn in place, marked, with nothing at the end", () => {
    // t2 was stopped before the provider logged it: sent with records 1–2 in
    // the session, so it sits before record 3 (t3's prompt).
    const items = [user("alpha", 1), agent("one", 2), user("gamma", 3), agent("three", 4)];
    const stopped = turn({
      turn_id: "t2",
      text: "stop me",
      position: 3,
      started_at: 1,
      ended_at: 2,
      outcome: "interrupted",
    });
    const out = mergeUserTurns(items, [
      echoed("t1", "alpha", 1),
      stopped,
      echoed("t3", "gamma", 3),
    ]);
    expect(shape(out)).toEqual([
      ["user_message", "alpha", "t1"],
      ["agent_message", "one", undefined],
      ["user_message", "stop me", "t2"],
      ["user_message", "gamma", "t3"],
      ["agent_message", "three", undefined],
    ]);
    expect(out[2]).toMatchObject({ undelivered: "interrupted", startedAt: 1, endedAt: 2 });
    expect(out[2]).not.toHaveProperty("recordSeq");
  });

  it("marks a turn dropped before it ran as not delivered, and a completed one not at all", () => {
    const items = [user("alpha", 1), agent("one", 2)];
    const failed = turn({
      turn_id: "f",
      text: "lost",
      position: 3,
      outcome: "failed",
      ended_at: 5,
    });
    const ran = turn({ turn_id: "c", text: "ran", position: 3, outcome: "completed", ended_at: 6 });
    const out = mergeUserTurns(items, [failed, ran]);
    expect(shape(out).slice(2)).toEqual([
      ["user_message", "lost", "f"],
      ["user_message", "ran", "c"],
    ]);
    expect(out[2]).toMatchObject({ undelivered: "failed" });
    expect(out[3]).not.toHaveProperty("undelivered");
  });

  it("inserts a prompt the adapter did not draw at its position", () => {
    // Record 3 is the prompt t2 paired with, but it reduced to no user_message.
    const items = [user("alpha", 1), agent("one", 2), agent("two", 3), agent("more", 4)];
    const out = mergeUserTurns(items, [echoed("t1", "alpha", 1), echoed("t2", "beta", 3)]);
    expect(shape(out)).toEqual([
      ["user_message", "alpha", "t1"],
      ["agent_message", "one", undefined],
      ["user_message", "beta", "t2"],
      ["agent_message", "two", undefined],
      ["agent_message", "more", undefined],
    ]);
    expect(out[2]).not.toHaveProperty("undelivered");
  });

  it("puts a turn past every record after its session's last item", () => {
    const items = [user("a", 1, PARENT), agent("b", 2, PARENT), user("c", 1)];
    const stopped = (session: string) =>
      turn({ turn_id: session, session_id: session, text: "x", position: 9, ended_at: 1 });
    expect(shape(mergeUserTurns(items, [stopped(PARENT)]))[2]).toEqual([
      "user_message",
      "x",
      PARENT,
    ]);
    expect(shape(mergeUserTurns(items, [stopped(OWN)]))[3]).toEqual(["user_message", "x", OWN]);
  });

  it("keeps an unmatched turn from shifting the turn ids of the ones around it", () => {
    // A prompt typed in the terminal (no row) and an echo-less turn between
    // two paired ones: each paired turn still lands on its own record.
    const items = [user("one", 1), user("native", 2), user("three", 4)];
    const out = mergeUserTurns(items, [
      echoed("t1", "one", 1),
      turn({ turn_id: "t2", text: "two", position: 4, ended_at: 1, outcome: "interrupted" }),
      echoed("t3", "three", 4),
    ]);
    expect(shape(out)).toEqual([
      ["user_message", "one", "t1"],
      ["user_message", "native", undefined],
      ["user_message", "two", "t2"],
      ["user_message", "three", "t3"],
    ]);
  });

  it("merges an inherited session and the own one without their seqs colliding", () => {
    // Both sessions number their records from 1.
    const items = [
      user("p1", 1, PARENT),
      agent("r1", 2, PARENT),
      user("c1", 1),
      agent("r2", 2),
      user("c2", 3),
    ];
    const out = mergeUserTurns(items, [
      { ...echoed("pt", "p1", 1, PARENT), inherited: true },
      echoed("ct1", "c1", 1),
      turn({ turn_id: "ct2", text: "stopped", position: 3, ended_at: 1, outcome: "interrupted" }),
      echoed("ct3", "c2", 3),
    ]);
    expect(shape(out)).toEqual([
      ["user_message", "p1", "pt"],
      ["agent_message", "r1", undefined],
      ["user_message", "c1", "ct1"],
      ["agent_message", "r2", undefined],
      ["user_message", "stopped", "ct2"],
      ["user_message", "c2", "ct3"],
    ]);
  });

  describe("a turn with no position", () => {
    const items = [user("alpha", 1), agent("one", 2)];

    it("draws the running turn at the end", () => {
      const running = turn({ turn_id: "r", text: "go", started_at: 7 });
      expect(mergeUserTurns(items, [running]).at(-1)).toEqual({
        kind: "user_message",
        text: "go",
        turnId: "r",
        startedAt: 7,
      });
    });

    it("draws a follow-up awaiting its echo as a queued bubble", () => {
      const followUp = turn({ turn_id: "q", text: "also" });
      expect(mergeUserTurns(items, [followUp]).at(-1)).toEqual({
        kind: "queued_message",
        text: "also",
        turnId: "q",
      });
    });

    it("keeps a send that never ran on screen, marked", () => {
      const dropped = turn({ turn_id: "d", text: "lost", outcome: "failed" });
      expect(mergeUserTurns(items, [dropped]).at(-1)).toMatchObject({
        kind: "user_message",
        turnId: "d",
        undelivered: "failed",
      });
    });

    it("adds nothing when the log already draws it", () => {
      const drawn: ChatItem[] = [...items, { kind: "user_message", text: "go", turnId: "r" }];
      const running = turn({ turn_id: "r", text: "go", started_at: 7 });
      expect(mergeUserTurns(drawn, [running])).toEqual(drawn);
    });

    it("adds nothing for a row from before positions that has ended", () => {
      const legacy = turn({ turn_id: "l", text: "alpha", started_at: 1, ended_at: 2 });
      expect(mergeUserTurns(items, [legacy])).toEqual(items);
    });
  });
});

describe("turnsOnPage", () => {
  const rec = (session: string, seq: number): SessionRecord => ({
    session_id: session,
    seq,
    provider: "claude",
    source: "transcript",
    native_id: `${session}-${seq}`,
    agent_version: null,
    body: {},
  });

  it("keeps the turns the page reaches, and every one with no position", () => {
    const page = [rec(PARENT, 40), rec(PARENT, 41), rec(OWN, 1)];
    const turns = [
      { ...echoed("old", "x", 12, PARENT), inherited: true },
      { ...echoed("edge", "x", 40, PARENT), inherited: true },
      echoed("own", "x", 1),
      turn({ turn_id: "live", started_at: 1 }),
    ];
    expect(turnsOnPage(turns, page).map((t) => t.turn_id)).toEqual(["edge", "own", "live"]);
  });

  it("drops an ancestor's turns when the page doesn't reach it", () => {
    const turns = [{ ...echoed("p", "x", 1, PARENT), inherited: true }, echoed("o", "x", 1)];
    expect(turnsOnPage(turns, [rec(OWN, 1)]).map((t) => t.turn_id)).toEqual(["o"]);
  });

  it("keeps the own session's turns when it has no records yet", () => {
    const stopped = turn({ turn_id: "s", position: 1, ended_at: 1, outcome: "interrupted" });
    expect(turnsOnPage([stopped], [rec(PARENT, 3)])).toEqual([stopped]);
  });
});

describe("mergeUserTurns over reduced records", () => {
  // The whole path: records → items stamped with (session, seq) → merge.
  const claude = (session: string, seq: number, body: Record<string, unknown>): SessionRecord => ({
    session_id: session,
    seq,
    provider: "claude",
    source: "transcript",
    native_id: `${session}-${seq}`,
    agent_version: null,
    body,
  });
  const said = (text: string) => ({ type: "user", message: { role: "user", content: text } });
  const reply = (text: string) => ({
    type: "assistant",
    message: { role: "assistant", content: [{ type: "text", text }] },
  });

  it("places turns across a fork whose sessions share seqs", () => {
    const records = [
      { ...claude(PARENT, 1, said("same")), inherited: true },
      { ...claude(PARENT, 2, reply("a")), inherited: true },
      claude(OWN, 1, said("same")),
      claude(OWN, 2, reply("b")),
    ];
    const out = mergeUserTurns(reduceRecords("claude", records), [
      { ...echoed("p", "same", 1, PARENT), inherited: true },
      echoed("o", "same", 1),
    ]);
    expect(shape(out)).toEqual([
      ["user_message", "same", "p"],
      ["agent_message", "a", undefined],
      ["user_message", "same", "o"],
      ["agent_message", "b", undefined],
    ]);
  });

  // Each prompt paired at the record the backend positions it on renders
  // exactly once, as its turn: the adapter's bubble carries that record.
  const recordsOf = (provider: string, bodies: Record<string, unknown>[]): SessionRecord[] =>
    bodies.map((body, i) => ({
      session_id: OWN,
      seq: i + 1,
      provider,
      source: "transcript",
      native_id: `n${i + 1}`,
      agent_version: null,
      body,
    }));
  const prompts = (items: ChatItem[]) =>
    items.flatMap((it) => (it.kind === "user_message" ? [[it.text, it.turnId]] : []));

  it.each(["codex/fixtures/rollout.jsonl", "codex/fixtures/rollout-0153.jsonl"])(
    "draws each codex prompt once (%s)",
    (fixture) => {
      const records = recordsOf("codex", readRecordFixture(fixture));
      const turns = codexPromptIndices(records.map((r) => r.body)).map((at, n) =>
        echoed(`t${n}`, "run echo hello", records[at].seq),
      );
      expect(turns.length).toBeGreaterThan(0);
      const out = mergeUserTurns(reduceRecords("codex", records), turns);
      expect(prompts(out)).toEqual(turns.map((t) => ["run echo hello", t.turn_id]));
    },
  );

  it("draws each opencode prompt once, its parts joined", () => {
    const records = recordsOf("opencode", [
      { id: "m1", role: "user", sessionID: "s" },
      { id: "p1", type: "text", messageID: "m1", text: "fix it" },
      { id: "p2", type: "text", messageID: "m1", text: "and test it" },
      { id: "m2", role: "assistant", sessionID: "s" },
      { id: "p3", type: "text", messageID: "m2", text: "done" },
      { id: "m3", role: "user", sessionID: "s" },
      { id: "p4", type: "text", messageID: "m3", text: "thanks" },
    ]);
    // `opencode_prompt_texts` positions each prompt on its user message blob.
    const out = mergeUserTurns(reduceRecords("opencode", records), [
      echoed("t1", "fix it", records[0].seq),
      echoed("t2", "thanks", records[5].seq),
    ]);
    expect(shape(out)).toEqual([
      ["user_message", "fix it\nand test it", "t1"],
      ["agent_message", "done", undefined],
      ["user_message", "thanks", "t2"],
    ]);
  });
});
