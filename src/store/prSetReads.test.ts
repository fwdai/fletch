// The focused-PR reads (`fetchPrLive`, `fetchPrState`, `fetchPrChecks`) write
// the legacy single-PR maps and, in the same write, the focused entry of the
// checkout's PR set — so the set doesn't go stale between the watcher's sweeps.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { getPrLive, getPrState, getPrChecks } = vi.hoisted(() => ({
  getPrLive: vi.fn(),
  getPrState: vi.fn(),
  getPrChecks: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { getPrLive, getPrState, getPrChecks } }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import type { PrChecks, PrSetEntry, PrState } from "@/api";
import { createGitSlice } from "./git";
import { resetPrWriteOrder, stampPrWrite } from "./prWriteOrder";
import type { AppState } from "./types";

const pr = (state: PrState["state"], number: number, title = "t"): PrState => ({
  number,
  url: `https://github.com/o/r/pull/${number}`,
  state,
  title,
  mergeable: "unknown",
});

const checks = (rollup: PrChecks["rollup"]): PrChecks => ({
  merge_state: "clean",
  rollup,
  total: 1,
  passed: rollup === "passing" ? 1 : 0,
  failed: rollup === "failing" ? 1 : 0,
  pending: 0,
  required_failing: [],
  runs: [],
});

const entry = (state: PrState, c: PrChecks | null = null): PrSetEntry => ({ state, checks: c });

/** A store whose checkout `a1` holds #651 and the focused #650. */
const makeStore = () => {
  const store = create<AppState>()(
    (...a) =>
      ({
        ...createGitSlice(...a),
        github: { authenticated: true },
        setLastError: vi.fn(),
      }) as unknown as AppState,
  );
  store.setState({
    prStates: { a1: pr("open", 650) },
    prChecks: { a1: checks("pending") },
    prSets: { a1: [entry(pr("open", 651), checks("passing")), entry(pr("open", 650))] },
  });
  return store;
};

beforeEach(() => {
  resetPrWriteOrder();
  getPrLive.mockReset();
  getPrState.mockReset();
  getPrChecks.mockReset();
});

describe("focused-PR reads keep the set's focused entry current", () => {
  it("fetchPrLive upserts the focused entry's state and checks, leaving siblings alone", async () => {
    const store = makeStore();
    getPrLive.mockResolvedValue({ state: pr("open", 650, "renamed"), checks: checks("failing") });
    await store.getState().fetchPrLive("a1");
    expect(store.getState().prSets.a1).toEqual([
      entry(pr("open", 651), checks("passing")),
      entry(pr("open", 650, "renamed"), checks("failing")),
    ]);
    expect(store.getState().prStates.a1?.title).toBe("renamed");
  });

  it("fetchPrState upserts the state and keeps the entry's checks", async () => {
    const store = makeStore();
    store.setState({
      prSets: { a1: [entry(pr("open", 651)), entry(pr("open", 650), checks("passing"))] },
    });
    getPrState.mockResolvedValue(pr("merged", 650));
    await store.getState().fetchPrState("a1");
    expect(store.getState().prSets.a1).toEqual([
      entry(pr("open", 651)),
      entry(pr("merged", 650), checks("passing")),
    ]);
  });

  it("fetchPrChecks writes the checks to the focused entry", async () => {
    const store = makeStore();
    getPrChecks.mockResolvedValue(checks("failing"));
    await store.getState().fetchPrChecks("a1");
    expect(store.getState().prChecks.a1).toEqual(checks("failing"));
    expect(store.getState().prSets.a1?.[1]).toEqual(entry(pr("open", 650), checks("failing")));
  });

  it("leaves the set alone on a confirmed 'no PR'", async () => {
    const store = makeStore();
    const before = store.getState().prSets;
    getPrState.mockResolvedValue(null);
    await store.getState().fetchPrState("a1");
    expect(store.getState().prStates.a1).toBeNull();
    expect(store.getState().prSets).toBe(before);
  });

  it("never files an unbound checkout's display-only merged PR as a set member", async () => {
    const store = makeStore();
    store.setState({ prStates: {}, prSets: {} });
    getPrState.mockResolvedValue(pr("merged", 12));
    await store.getState().fetchPrState("a1");
    expect(store.getState().prStates.a1?.number).toBe(12);
    expect(store.getState().prSets.a1).toBeUndefined();
  });

  it("drops the whole write when a newer one already landed", async () => {
    const store = makeStore();
    const before = store.getState().prSets;
    getPrLive.mockImplementation(async () => {
      // The watcher's event lands while the read is in flight.
      stampPrWrite("prStates", "a1");
      stampPrWrite("prChecks", "a1");
      return { state: pr("open", 650, "stale"), checks: checks("failing") };
    });
    await store.getState().fetchPrLive("a1");
    expect(store.getState().prStates.a1?.title).toBe("t");
    expect(store.getState().prSets).toBe(before);
  });
});
