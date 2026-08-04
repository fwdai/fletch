import { describe, expect, it } from "vitest";
import type { AgentRecord, UserTurn } from "@/api";
import { chatActiveAt, type StandupSignals, shouldAskForStandup } from "./standup";

/** A board that moved an hour after the chat was last live — the case the
 *  digest exists for. */
function signals(over: Partial<StandupSignals> = {}): StandupSignals {
  return {
    boardMovedAt: 2_000,
    chatActiveAt: 1_000,
    freshlySpawned: false,
    alreadyAsked: false,
    ...over,
  };
}

function turn(over: Partial<UserTurn> = {}): UserTurn {
  return {
    turn_id: "t1",
    seq: 1,
    text: "hi",
    attachments: [],
    native_id: null,
    started_at: null,
    ended_at: null,
    ...over,
  };
}

const chat = { created_at: "2026-08-04T10:00:00.000Z" } as Pick<AgentRecord, "created_at">;

describe("shouldAskForStandup", () => {
  it("asks when the board moved after the chat was last live", () => {
    expect(shouldAskForStandup(signals())).toBe(true);
  });

  it("stays quiet when nothing has happened since", () => {
    expect(shouldAskForStandup(signals({ boardMovedAt: 1_000 }))).toBe(false);
    expect(shouldAskForStandup(signals({ boardMovedAt: 999 }))).toBe(false);
  });

  it("stays quiet on a board with no history at all", () => {
    // Nothing has ever happened, so there is nothing to summarize — and asking
    // would train the user to ignore the next one.
    expect(shouldAskForStandup(signals({ boardMovedAt: null }))).toBe(false);
  });

  it("never fires on a chat spawned in this session", () => {
    // Its opening turn *is* the conversation: there is no "since we last spoke",
    // however old the board's last movement is.
    expect(shouldAskForStandup(signals({ freshlySpawned: true }))).toBe(false);
  });

  it("fires at most once per project per app session", () => {
    expect(shouldAskForStandup(signals({ alreadyAsked: true }))).toBe(false);
  });

  it("weighs the guards independently", () => {
    // Every combination that has any blocker set stays quiet; only the all-clear
    // case asks.
    for (const boardMovedAt of [null, 500, 2_000]) {
      for (const freshlySpawned of [false, true]) {
        for (const alreadyAsked of [false, true]) {
          const clear = boardMovedAt === 2_000 && !freshlySpawned && !alreadyAsked;
          expect(shouldAskForStandup(signals({ boardMovedAt, freshlySpawned, alreadyAsked }))).toBe(
            clear,
          );
        }
      }
    }
  });
});

describe("chatActiveAt", () => {
  it("takes the newest turn's end", () => {
    expect(
      chatActiveAt(chat, [
        turn({ turn_id: "a", started_at: 100, ended_at: 200 }),
        turn({ turn_id: "b", started_at: 300, ended_at: 400 }),
      ]),
    ).toBe(400);
  });

  it("falls back to the start of a turn still in flight", () => {
    expect(chatActiveAt(chat, [turn({ started_at: 300, ended_at: null })])).toBe(300);
  });

  it("skips a turn that never started and keeps looking", () => {
    // A failed send awaiting retry has no timestamps; counting it as "now" would
    // suppress the digest forever.
    expect(
      chatActiveAt(chat, [
        turn({ turn_id: "a", started_at: 100, ended_at: 200 }),
        turn({ turn_id: "b", started_at: null, ended_at: null }),
      ]),
    ).toBe(200);
  });

  it("falls back to when the chat was created", () => {
    // A chat the user opened and never typed in: the last moment it can be said
    // to have been in sync with the board.
    expect(chatActiveAt(chat, [])).toBe(Date.parse("2026-08-04T10:00:00.000Z"));
    expect(chatActiveAt(chat, [turn()])).toBe(Date.parse("2026-08-04T10:00:00.000Z"));
  });

  it("reads an undateable chat as epoch zero", () => {
    // Any board movement then counts as newer — the safe direction, since a chat
    // we can't date is one we can't claim is current.
    expect(chatActiveAt({ created_at: "not a date" }, [])).toBe(0);
  });
});
