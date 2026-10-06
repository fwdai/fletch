import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { setFocusedPr } = vi.hoisted(() => ({ setFocusedPr: vi.fn() }));
vi.mock("@/api", () => ({ api: { setFocusedPr } }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import type { PrState } from "@/api";
import { type EnvironmentEntry, LOCAL_ENVIRONMENT_ID, setEnvironmentsSource } from "./environments";
import { GATES } from "./gates";
import { createGitSlice } from "./git";
import type { AppState } from "./types";

const pr = (number: number): PrState => ({
  number,
  url: `https://github.com/o/r/pull/${number}`,
  state: "open",
  title: `PR ${number}`,
  mergeable: "unknown",
});

const activeIs = (entry?: EnvironmentEntry) =>
  setEnvironmentsSource(() => ({
    activeEnvironmentId: entry?.id ?? LOCAL_ENVIRONMENT_ID,
    environments: entry ? { [entry.id]: entry } : {},
  }));

const makeStore = () => {
  const setLastError = vi.fn();
  const store = create<AppState>()(
    (...a) =>
      ({
        ...createGitSlice(...a),
        setLastError,
        github: { authenticated: true },
      }) as unknown as AppState,
  );
  return { store, setLastError };
};

beforeEach(() => {
  setFocusedPr.mockReset();
  activeIs();
});

/** The focus swap itself is `applyPrStateChanged`'s (see prEvents.test.ts):
 *  `focusPr` asks the host and applies its answer as the host's event would. */
describe("focusPr", () => {
  it("asks the host to move the focus and follows its answer", async () => {
    const { store, setLastError } = makeStore();
    store.setState({
      prStates: { "a1::api": pr(43) },
      prChecks: { "a1::api": null },
      prComments: { "a1::api": { unresolved: [] } },
      prSets: { "a1::api": [{ state: pr(43), checks: null }] },
    });
    setFocusedPr.mockResolvedValue(pr(42));

    await store.getState().focusPr("a1", 42, "api");

    expect(setFocusedPr).toHaveBeenCalledWith("a1", 42, "api");
    expect(setLastError).not.toHaveBeenCalled();
    // Awaiting callers read the new focus even if the host's event is late.
    expect(store.getState().prStates["a1::api"]?.number).toBe(42);
    expect("a1::api" in store.getState().prComments).toBe(false);
  });

  it("reports a refusal", async () => {
    const { store, setLastError } = makeStore();
    setFocusedPr.mockRejectedValue(new Error("not in set"));

    await store.getState().focusPr("a1", 42);

    expect(setLastError).toHaveBeenCalledWith("Error: not in set");
  });

  it("refuses on a host without the op", async () => {
    activeIs({
      id: "host-1",
      name: "Cloud box",
      kind: "remote",
      connection: "connected",
      protocol: { version: 2, ops: [], events: [], features: [] },
    });
    const { store, setLastError } = makeStore();

    await store.getState().focusPr("a1", 42);

    expect(setLastError).toHaveBeenCalledWith(GATES.focusPr.reason);
    expect(setFocusedPr).not.toHaveBeenCalled();
  });
});
