import { describe, expect, it } from "vitest";
import type { ChatItem, TurnTiming } from "../../../adapters";
import { planTurnFooters } from "./plan";

const done = (activeMs: number): TurnTiming => ({ activeMs, runningSince: null, completedAt: 1 });
const open: TurnTiming = { activeMs: 0, runningSince: 5_000, completedAt: null };

const user = (timing?: TurnTiming): ChatItem => ({ kind: "user_message", text: "go", timing });
const agent: ChatItem = { kind: "agent_message", text: "ok" };
const queued: ChatItem = { kind: "queued_message", text: "next" };

describe("planTurnFooters", () => {
  it("places a completed turn's footer after its last item", () => {
    const items = [user(done(38_000)), agent];
    const { completedFooters } = planTurnFooters(items);
    expect([...completedFooters]).toEqual([[1, 38]]);
  });

  it("closes each turn before the next user message", () => {
    const items = [user(done(5_000)), agent, user(done(10_000)), agent];
    const { completedFooters } = planTurnFooters(items);
    expect([...completedFooters]).toEqual([
      [1, 5],
      [3, 10],
    ]);
  });

  it("does not render a static footer for the open in-flight turn", () => {
    const items = [user(done(5_000)), agent, user(open), agent];
    const { completedFooters, openTurnTiming } = planTurnFooters(items);
    expect([...completedFooters]).toEqual([[1, 5]]);
    expect(openTurnTiming).toBe(open);
  });

  it("places a completed turn's footer before a trailing queued bubble", () => {
    // After a turn ends, a carried-forward follow-up is appended. Its bubble
    // belongs to the next turn, so the footer must stay above it.
    const items = [user(done(38_000)), agent, queued];
    const { completedFooters, openTurnTiming } = planTurnFooters(items);
    expect([...completedFooters]).toEqual([[1, 38]]); // after `agent`, not `queued`
    expect(openTurnTiming).toBeUndefined();
  });

  it("places a completed turn's footer before a queued bubble between turns", () => {
    const items = [user(done(5_000)), agent, queued, user(open)];
    const { completedFooters } = planTurnFooters(items);
    expect([...completedFooters]).toEqual([[1, 5]]); // after `agent`, not `queued`
  });

  it("keeps the live timer when a follow-up is queued mid-turn", () => {
    const items = [user(open), agent, queued];
    const { completedFooters, openTurnTiming } = planTurnFooters(items);
    expect(completedFooters.size).toBe(0);
    expect(openTurnTiming).toBe(open);
  });
});
