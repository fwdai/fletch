// A turn's usage reaches the store while the turn runs — off the live stream —
// and is handed back to the records once they hold it, without being counted
// twice on the way.

import { describe, expect, it, vi } from "vitest";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/storage/db", () => ({ dbUpsert: async () => "ok" }));

import type { RawEvent } from "@/adapters/types";
import { usageFromRecords } from "@/adapters/usage";
import type { SessionRecord } from "@/api";
import { foldLiveUsage, settleLiveUsage } from "@/helpers/usage";
import type { AppState } from "@/store";

const call = (id: string, input: number, cacheRead: number): RawEvent =>
  ({
    type: "assistant",
    message: { id, usage: { input_tokens: input, cache_read_input_tokens: cacheRead } },
  }) as RawEvent;

const records = (...bodies: RawEvent[]): SessionRecord[] =>
  bodies.map((body, seq) => ({
    session_id: "s1",
    seq,
    provider: "claude",
    source: "transcript",
    native_id: `n${seq}`,
    agent_version: null,
    body,
  }));

/** A store holding one agent, with the two accessors the helpers take. */
function store(provider: string) {
  let state = {
    usage: {},
    workspace: { agents: [{ id: "a1", provider }] },
    offSidebarAgents: {},
  } as unknown as AppState;
  const get = () => state;
  const set = (patch: Partial<AppState>) => {
    state = { ...state, ...patch };
  };
  return { get, set, fold: (ev: RawEvent) => foldLiveUsage(get, set, "a1", ev) };
}

describe("foldLiveUsage", () => {
  it("measures the window from the first call of a first turn", () => {
    const { get, fold } = store("claude");
    fold(call("m1", 5, 8000));
    expect(get().usage.a1.context).toMatchObject({ state: "measured", tokens: 8005 });
    fold(call("m2", 7, 9000));
    expect(get().usage.a1.context.tokens).toBe(9007);
    expect(get().usage.a1.spend.tokens.cacheRead).toBe(17_000);
    settleLiveUsage("a1");
  });

  it("starts over from the records once they hold the turn", () => {
    const { get, set, fold } = store("claude");
    fold(call("m1", 5, 8000));

    // Turn end: the records carry the same call, and replace the live fold.
    const recorded = usageFromRecords("claude", records(call("m1", 5, 8000)));
    settleLiveUsage("a1");
    set({ usage: { a1: recorded } });

    fold(call("m2", 7, 9000));
    expect(get().usage.a1.spend.tokens).toMatchObject({ input: 12, cacheRead: 17_000 });
    settleLiveUsage("a1");
  });

  it("ignores events that report no call, and agents whose stream reports none", () => {
    const claude = store("claude");
    claude.fold({ type: "result", usage: { input_tokens: 99 } } as RawEvent);
    expect(claude.get().usage).toEqual({});

    const codex = store("codex");
    codex.fold(call("m1", 5, 8000));
    expect(codex.get().usage).toEqual({});
  });
});
