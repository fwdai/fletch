import { describe, expect, it } from "vitest";
import type { ChatItem, TurnTiming } from "./adapters";
import { freezeOpenTurnTiming, resumeOpenTurnTiming } from "./helpers";

const turn = (timing: TurnTiming): ChatItem => ({ kind: "user_message", text: "go", timing });
const reply: ChatItem = { kind: "agent_message", text: "ok" };
const timingOf = (items: ChatItem[], i: number) =>
  (items[i] as Extract<ChatItem, { kind: "user_message" }>).timing;

describe("freezeOpenTurnTiming", () => {
  it("folds the open span into activeMs and clears runningSince", () => {
    const items = [turn({ activeMs: 1_000, runningSince: 5_000, completedAt: null }), reply];
    const out = freezeOpenTurnTiming(items, 9_000);
    expect(timingOf(out, 0)).toEqual({ activeMs: 5_000, runningSince: null, completedAt: null });
  });

  it("is a no-op when already paused", () => {
    const items = [turn({ activeMs: 3_000, runningSince: null, completedAt: null })];
    expect(freezeOpenTurnTiming(items, 9_000)).toBe(items);
  });

  it("is a no-op when the turn is completed", () => {
    const items = [turn({ activeMs: 3_000, runningSince: null, completedAt: 8_000 })];
    expect(freezeOpenTurnTiming(items, 9_000)).toBe(items);
  });

  it("only touches the latest (open) turn", () => {
    const items = [
      turn({ activeMs: 2_000, runningSince: null, completedAt: 1_000 }),
      reply,
      turn({ activeMs: 0, runningSince: 5_000, completedAt: null }),
    ];
    const out = freezeOpenTurnTiming(items, 7_000);
    expect(timingOf(out, 0)).toEqual({ activeMs: 2_000, runningSince: null, completedAt: 1_000 });
    expect(timingOf(out, 2)).toEqual({ activeMs: 2_000, runningSince: null, completedAt: null });
  });
});

describe("resumeOpenTurnTiming", () => {
  it("restarts the clock from now on a paused turn", () => {
    const items = [turn({ activeMs: 5_000, runningSince: null, completedAt: null })];
    const out = resumeOpenTurnTiming(items, 10_000);
    expect(timingOf(out, 0)).toEqual({ activeMs: 5_000, runningSince: 10_000, completedAt: null });
  });

  it("is a no-op when the turn is already running", () => {
    const items = [turn({ activeMs: 0, runningSince: 3_000, completedAt: null })];
    expect(resumeOpenTurnTiming(items, 10_000)).toBe(items);
  });
});
