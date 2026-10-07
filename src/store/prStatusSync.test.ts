// Fleet PR seeds share one request per environment. A launch/reconnect may ask
// for the stronger closed-PR recheck while a cheap focus seed is still out; the
// strong caller must wait for its own live read, never inherit the weak one.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { getAllPrStatus } = vi.hoisted(() => ({ getAllPrStatus: vi.fn() }));
vi.mock("@/api", () => ({ api: { getAllPrStatus } }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import { createEnvironmentsSlice, setEnvironmentsSource } from "./environments";
import { createGitSlice } from "./git";
import type { AppState } from "./types";

const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
};

const makeStore = () => {
  const store = create<AppState>()(
    (...a) =>
      ({
        ...createEnvironmentsSlice(...a),
        ...createGitSlice(...a),
        github: { authenticated: true },
      }) as unknown as AppState,
  );
  setEnvironmentsSource(store.getState);
  return store;
};

describe("fleet PR status synchronization", () => {
  beforeEach(() => getAllPrStatus.mockReset());

  it("upgrades a weak in-flight seed when a closed-PR recheck joins it", async () => {
    const weakAnswer = deferred<Record<string, never>>();
    getAllPrStatus.mockReturnValueOnce(weakAnswer.promise).mockResolvedValueOnce({});
    const store = makeStore();

    const weak = store.getState().loadAllPrStatus();
    const strong = store.getState().loadAllPrStatus(true);
    expect(getAllPrStatus).toHaveBeenCalledTimes(1);
    expect(getAllPrStatus).toHaveBeenNthCalledWith(1, false);

    weakAnswer.resolve({});
    await weak;
    await strong;
    expect(getAllPrStatus).toHaveBeenCalledTimes(2);
    expect(getAllPrStatus).toHaveBeenNthCalledWith(2, true);
  });

  it("lets a strong in-flight seed satisfy weaker concurrent callers", async () => {
    const answer = deferred<Record<string, never>>();
    getAllPrStatus.mockReturnValue(answer.promise);
    const store = makeStore();

    const strong = store.getState().loadAllPrStatus(true);
    const weak = store.getState().loadAllPrStatus();
    expect(getAllPrStatus).toHaveBeenCalledTimes(1);
    expect(getAllPrStatus).toHaveBeenCalledWith(true);

    answer.resolve({});
    await Promise.all([strong, weak]);
  });

  it("does not retarget a queued upgrade after an environment switch", async () => {
    const oldAnswer = deferred<Record<string, never>>();
    const newAnswer = deferred<Record<string, never>>();
    getAllPrStatus.mockReturnValueOnce(oldAnswer.promise).mockReturnValueOnce(newAnswer.promise);
    const store = makeStore();

    const oldWeak = store.getState().loadAllPrStatus();
    const oldStrong = store.getState().loadAllPrStatus(true);
    store.setState((s) => ({
      activeEnvironmentId: "other-local",
      environments: {
        ...s.environments,
        "other-local": {
          id: "other-local",
          name: "Other local",
          kind: "local",
          connection: "connected",
        },
      },
    }));
    const newStrong = store.getState().loadAllPrStatus(true);
    expect(getAllPrStatus).toHaveBeenCalledTimes(2);

    oldAnswer.resolve({});
    newAnswer.resolve({});
    await Promise.all([oldWeak, oldStrong, newStrong]);
    expect(getAllPrStatus).toHaveBeenCalledTimes(2);
  });

  it("coalesces equal requests and starts fresh after they settle", async () => {
    getAllPrStatus.mockResolvedValue({});
    const store = makeStore();

    await Promise.all([store.getState().loadAllPrStatus(), store.getState().loadAllPrStatus()]);
    await store.getState().loadAllPrStatus();

    expect(getAllPrStatus).toHaveBeenCalledTimes(2);
    expect(getAllPrStatus).toHaveBeenNthCalledWith(1, false);
    expect(getAllPrStatus).toHaveBeenNthCalledWith(2, false);
  });
});
