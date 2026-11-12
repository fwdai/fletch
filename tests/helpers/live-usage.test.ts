// A turn's usage reaches the store while the turn runs — off the live stream —
// and is handed back to the records once they hold it, without being counted
// twice on the way and without losing what streamed in meanwhile.

import { describe, expect, it, vi } from "vitest";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/storage/db", () => ({ dbUpsert: async () => "ok" }));

import type { RawEvent } from "@/adapters/types";
import { usageFromRecords } from "@/adapters/usage";
import type { SessionRecord } from "@/api";
import { beginUsageRead, foldLiveUsage, settleLiveUsage, type UsageRead } from "@/helpers/usage";
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
function store(id: string, agent: { provider: string; model?: string; created_at?: string }) {
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
    /** A records read starting. */
    read: () => beginUsageRead(id),
    /** That read landing with `bodies`: its snapshot settled, and stored unless
     *  it is to be left alone. */
    land: (read: UsageRead, ...bodies: RawEvent[]) => {
      const settled = settleLiveUsage(id, recorded(...bodies), read);
      if (settled) set({ usage: { [id]: settled } });
    },
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

  // Agent ids are place names from a recycled pool: a discarded agent's name
  // comes back on a new one, which must not inherit what the old one streamed.
  it("starts a recycled id's agent from nothing", () => {
    const old = store("lima", { provider: "claude", model: "old-model", created_at: "t1" });
    old.fold(call("m1", 5, 8000));

    const next = store("lima", { provider: "claude", model: "new-model", created_at: "t2" });
    next.fold(call("m9", 7, 9000));
    expect(next.usage().spend.tokens).toMatchObject({ input: 7, cacheRead: 9000 });
    expect(next.usage().context).toMatchObject({ tokens: 9007, model: "new-model" });
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
    s.land(s.read(), call("m1", 5, 8000));
    expect(s.usage().spend.tokens).toMatchObject({ input: 5, cacheRead: 8000 });

    s.fold(call("m2", 7, 9000));
    expect(s.usage().spend.tokens).toMatchObject({ input: 12, cacheRead: 17_000 });
  });

  // The records read is async. A call that streams in while it runs is not in
  // what it returns, and must not vanish when the read lands.
  it("keeps a call that streamed in while the records were being read", () => {
    const s = store("race", { provider: "claude" });
    s.fold(call("m1", 5, 8000));

    const read = s.read(); // the refresh starts reading…
    s.fold(call("m2", 7, 9000)); // …and the next call streams in meanwhile
    s.land(read, call("m1", 5, 8000)); // the read returns m1 only

    expect(s.usage().context.tokens).toBe(9007);
    expect(s.usage().spend.tokens).toMatchObject({ input: 12, cacheRead: 17_000 });

    // The following refresh holds both, and nothing is left to fold over it.
    s.land(s.read(), call("m1", 5, 8000), call("m2", 7, 9000));
    expect(s.usage().spend.tokens).toMatchObject({ input: 12, cacheRead: 17_000 });
  });

  // Every records-appended event starts its own read, so two can overlap and
  // the one that started first — holding fewer records — can land last.
  it("ignores an older read that lands after a newer one", () => {
    const s = store("overlap", { provider: "claude" });
    s.fold(call("m1", 5, 8000));
    const first = s.read();
    s.fold(call("m2", 7, 9000));
    const second = s.read();

    s.land(second, call("m1", 5, 8000), call("m2", 7, 9000));
    s.land(first, call("m1", 5, 8000));

    expect(s.usage().context.tokens).toBe(9007);
    expect(s.usage().spend.tokens).toMatchObject({ input: 12, cacheRead: 17_000 });
  });

  // Not every change to the records comes with a live call: a compaction is
  // transcript-only. The older read must lose on its place among the reads,
  // not on how far the live stream had got — that is the same for both here.
  it("ignores an older read even when no call streamed between the two", () => {
    const s = store("compaction", { provider: "claude" });
    s.fold(call("m1", 5, 8000));
    const first = s.read();
    const second = s.read();
    const boundary = {
      type: "system",
      subtype: "compact_boundary",
      compactMetadata: { preTokens: 8005 },
    } as RawEvent;

    s.land(second, call("m1", 5, 8000), boundary);
    s.land(first, call("m1", 5, 8000));

    expect(s.usage().context).toMatchObject({ state: "reset", compacted: { freed: null } });
  });

  it("orders reads for an agent that never streamed a call, too", () => {
    const s = store("records-only", { provider: "codex" });
    const first = s.read();
    const second = s.read();
    const newer = recorded(call("m1", 5, 8000), call("m2", 7, 9000));

    expect(settleLiveUsage("records-only", newer, second)).toBe(newer);
    expect(settleLiveUsage("records-only", recorded(call("m1", 5, 8000)), first)).toBeNull();
    expect(s.usage()).toBeUndefined();
  });
});
