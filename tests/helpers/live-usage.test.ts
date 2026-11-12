// A turn's usage reaches the store while the turn runs — off the live stream —
// and is handed back to the records once they hold it, without being counted
// twice on the way and without losing what streamed in meanwhile.

import { describe, expect, it, vi } from "vitest";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/storage/db", () => ({ dbUpsert: async () => "ok" }));

import type { RawEvent } from "@/adapters/types";
import { usageFromRecords } from "@/adapters/usage";
import type { SessionRecord } from "@/api";
import { foldLiveUsage, liveUsageMark, settleLiveUsage } from "@/helpers/usage";
import type { AppState } from "@/store";

const call = (id: string, input: number, cacheRead: number): RawEvent =>
  ({
    type: "assistant",
    message: { id, usage: { input_tokens: input, cache_read_input_tokens: cacheRead } },
  }) as RawEvent;

/** What the records give once they hold `bodies`. */
const recorded = (...bodies: RawEvent[]) =>
  usageFromRecords(
    "claude",
    bodies.map(
      (body, seq): SessionRecord => ({
        session_id: "s1",
        seq,
        provider: "claude",
        source: "transcript",
        native_id: `n${seq}`,
        agent_version: null,
        body,
      }),
    ),
  );

/** A store holding one agent, driven the way the event listeners drive it. The
 *  live turns are module state, so every test brings an agent of its own. */
function store(id: string, agent: { provider: string; model?: string }) {
  let state = {
    usage: {},
    workspace: { agents: [{ id, ...agent }] },
    offSidebarAgents: {},
  } as unknown as AppState;
  const get = () => state;
  const set = (patch: Partial<AppState>) => {
    state = { ...state, ...patch };
  };
  return {
    usage: () => state.usage[id],
    fold: (ev: RawEvent) => foldLiveUsage(get, set, id, ev),
    mark: () => liveUsageMark(id),
    /** A records refresh landing: the snapshot it read, settled and stored. */
    refresh: (mark: number, ...bodies: RawEvent[]) =>
      set({ usage: { [id]: settleLiveUsage(id, recorded(...bodies), mark) } }),
  };
}

describe("foldLiveUsage", () => {
  it("measures the window from the first call of a first turn", () => {
    const s = store("first", { provider: "claude" });
    s.fold(call("m1", 5, 8000));
    expect(s.usage().context).toMatchObject({ state: "measured", tokens: 8005 });
    s.fold(call("m2", 7, 9000));
    expect(s.usage().context.tokens).toBe(9007);
    expect(s.usage().spend.tokens.cacheRead).toBe(17_000);
  });

  // The gauge sized its window by the spawn model before the call; a call that
  // names no model must not shrink it back to the default.
  it("keeps the spawn model's window when a first call names no model", () => {
    const s = store("model", { provider: "claude", model: "claude-opus-4-8[1m]" });
    s.fold(call("m1", 5, 8000));
    expect(s.usage().context).toMatchObject({ state: "measured", model: "claude-opus-4-8[1m]" });
  });

  it("ignores events that report no call, and agents whose stream reports none", () => {
    const claude = store("quiet", { provider: "claude" });
    claude.fold({ type: "result", usage: { input_tokens: 99 } } as RawEvent);
    expect(claude.usage()).toBeUndefined();

    const codex = store("codex", { provider: "codex" });
    codex.fold(call("m1", 5, 8000));
    expect(codex.usage()).toBeUndefined();
  });
});

describe("settleLiveUsage", () => {
  it("starts over from the records once they hold the turn", () => {
    const s = store("settle", { provider: "claude" });
    s.fold(call("m1", 5, 8000));

    // Turn end: the records carry the same call, and replace the live fold.
    s.refresh(s.mark(), call("m1", 5, 8000));
    expect(s.usage().spend.tokens).toMatchObject({ input: 5, cacheRead: 8000 });

    s.fold(call("m2", 7, 9000));
    expect(s.usage().spend.tokens).toMatchObject({ input: 12, cacheRead: 17_000 });
  });

  // The records read is async. A call that streams in while it runs is not in
  // what it returns, and must not vanish when the read lands.
  it("keeps a call that streamed in while the records were being read", () => {
    const s = store("race", { provider: "claude" });
    s.fold(call("m1", 5, 8000));

    const mark = s.mark(); // the refresh starts reading…
    s.fold(call("m2", 7, 9000)); // …and the next call streams in meanwhile
    s.refresh(mark, call("m1", 5, 8000)); // the read returns m1 only

    expect(s.usage().context.tokens).toBe(9007);
    expect(s.usage().spend.tokens).toMatchObject({ input: 12, cacheRead: 17_000 });

    // The following refresh holds both, and nothing is left to fold over it.
    s.refresh(s.mark(), call("m1", 5, 8000), call("m2", 7, 9000));
    expect(s.usage().spend.tokens).toMatchObject({ input: 12, cacheRead: 17_000 });
  });

  it("hands back the records' snapshot untouched when nothing was streamed", () => {
    const snapshot = recorded(call("m1", 5, 8000));
    expect(settleLiveUsage("never-streamed", snapshot, liveUsageMark("never-streamed"))).toBe(
      snapshot,
    );
  });
});
