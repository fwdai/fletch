// `focusPr` is the PR switcher's one write. It must move the panel at once (the
// legacy maps take the chosen PR from the set), leave the focused one-shot reads
// something to do (threads dropped), not be rolled back by a read issued before
// the switch, and on a refusal say so and re-read the host's focus.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { setFocusedPr, getPrLive } = vi.hoisted(() => ({
  setFocusedPr: vi.fn(),
  getPrLive: vi.fn(),
}));
vi.mock("@/api", () => ({ api: { setFocusedPr, getPrLive } }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import type { PrChecks, PrSetEntry, PrState } from "@/api";
import { type EnvironmentEntry, LOCAL_ENVIRONMENT_ID, setEnvironmentsSource } from "./environments";
import { GATES } from "./gates";
import { createGitSlice } from "./git";
import { acceptPrWrite, issuePrWrite, resetPrWriteOrder } from "./prWriteOrder";
import type { AppState } from "./types";

const pr = (number: number, over: Partial<PrState> = {}): PrState => ({
  number,
  url: `https://github.com/o/r/pull/${number}`,
  state: "open",
  title: `PR ${number}`,
  mergeable: "unknown",
  branch: `feat/${number}`,
  ...over,
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

const activeIs = (entry?: EnvironmentEntry) =>
  setEnvironmentsSource(() => ({
    activeEnvironmentId: entry?.id ?? LOCAL_ENVIRONMENT_ID,
    environments: entry ? { [entry.id]: entry } : {},
  }));

/** A checkout focused on #43 (failing CI, two threads) that also holds #42. */
const makeStore = (set: PrSetEntry[]) => {
  const setLastError = vi.fn();
  const store = create<AppState>()(
    (...a) =>
      ({
        ...createGitSlice(...a),
        setLastError,
        github: { authenticated: true },
      }) as unknown as AppState,
  );
  store.setState({
    prStates: { a1: pr(43) },
    prChecks: { a1: checks("failing") },
    prComments: { a1: { unresolved: [] } },
    prSets: { a1: set },
  });
  return { store, setLastError };
};

beforeEach(() => {
  setFocusedPr.mockReset();
  getPrLive.mockReset();
  resetPrWriteOrder();
  activeIs();
});

describe("focusPr", () => {
  it("moves the focused maps to the chosen PR and asks the host", async () => {
    const { store } = makeStore([
      { state: pr(43), checks: checks("failing") },
      { state: pr(42), checks: checks("passing") },
    ]);
    setFocusedPr.mockResolvedValue(pr(42));

    await store.getState().focusPr("a1", 42);

    const s = store.getState();
    expect(s.prStates.a1?.number).toBe(42);
    expect(s.prChecks.a1?.rollup).toBe("passing");
    expect("a1" in s.prComments).toBe(false);
    expect(setFocusedPr).toHaveBeenCalledWith("a1", 42, undefined);
  });

  it("drops checks the set has nothing to say about rather than keep the last PR's", async () => {
    const { store } = makeStore([
      { state: pr(43), checks: checks("failing") },
      { state: pr(42, { state: "merged" }), checks: null },
    ]);
    setFocusedPr.mockResolvedValue(pr(42));

    await store.getState().focusPr("a1", 42);

    expect("a1" in store.getState().prChecks).toBe(false);
  });

  it("outranks a read issued before the switch", async () => {
    const { store } = makeStore([
      { state: pr(43), checks: null },
      { state: pr(42), checks: null },
    ]);
    setFocusedPr.mockResolvedValue(pr(42));
    const before = issuePrWrite();

    await store.getState().focusPr("a1", 42);

    expect(acceptPrWrite("prStates", "a1", before)).toBe(false);
  });

  it("reports a refusal and re-reads the host's focus", async () => {
    const { store, setLastError } = makeStore([
      { state: pr(43), checks: null },
      { state: pr(42), checks: null },
    ]);
    setFocusedPr.mockRejectedValue(new Error("not in set"));
    getPrLive.mockResolvedValue({ state: pr(43), checks: null });

    await store.getState().focusPr("a1", 42);

    expect(setLastError).toHaveBeenCalledWith("Error: not in set");
    expect(store.getState().prStates.a1?.number).toBe(43);
  });

  it("refuses on a host without the op, touching nothing", async () => {
    activeIs({
      id: "host-1",
      name: "Cloud box",
      kind: "remote",
      connection: "connected",
      protocol: { version: 2, ops: [], events: [], features: [] },
    });
    const { store, setLastError } = makeStore([
      { state: pr(43), checks: null },
      { state: pr(42), checks: null },
    ]);

    await store.getState().focusPr("a1", 42);

    expect(setLastError).toHaveBeenCalledWith(GATES.focusPr.reason);
    expect(setFocusedPr).not.toHaveBeenCalled();
    expect(store.getState().prStates.a1?.number).toBe(43);
  });
});
