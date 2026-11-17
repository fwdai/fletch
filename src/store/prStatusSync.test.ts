// Fleet PR seeds share one request per host. A launch/reconnect may ask
// for the stronger closed-PR recheck while a cheap focus seed is still out; the
// strong caller must wait for its own live read, never inherit the weak one.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

const { getAllPrStatus } = vi.hoisted(() => ({ getAllPrStatus: vi.fn() }));
vi.mock("@/api", () => ({ api: { getAllPrStatus } }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import { testEngine } from "@/test/engine";
import {
  createEnvironmentsSlice,
  LOCAL_ENVIRONMENT_ID,
  setEnvironmentsSource,
} from "./environments";
import { createGitSlice } from "./git";
import type { AppState } from "./types";

const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
};

/** A store acting for `hostId`: this Mac's unless told otherwise. */
const makeStore = (hostId = LOCAL_ENVIRONMENT_ID) => {
  const store = create<AppState>()(
    (set, get) =>
      ({
        ...createEnvironmentsSlice(set, get, testEngine(hostId)),
        ...createGitSlice(set, get, testEngine(hostId)),
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

  it("keeps each host's seeds apart", async () => {
    // A seed in flight on one host is nothing to another's: neither joins the
    // other's read, and a queued upgrade stays with the host that asked.
    const localAnswer = deferred<Record<string, never>>();
    getAllPrStatus.mockReturnValueOnce(localAnswer.promise).mockResolvedValue({});
    const local = makeStore();
    const host = makeStore("host-1");

    const weak = local.getState().loadAllPrStatus();
    await host.getState().loadAllPrStatus();
    expect(getAllPrStatus).toHaveBeenCalledTimes(2);

    const strong = local.getState().loadAllPrStatus(true);
    localAnswer.resolve({});
    await Promise.all([weak, strong]);
    expect(getAllPrStatus).toHaveBeenCalledTimes(3);
    expect(getAllPrStatus).toHaveBeenNthCalledWith(3, true);
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
