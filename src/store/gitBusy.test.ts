// The Git panel's busy lock and outcome notice: one action per checkout per
// host, held in the store so it outlives the panel section that started it
// (the section mounts per checkout), keyed by the host it began on, and
// released only by its holder.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import { LOCAL_ENVIRONMENT_ID, setEnvironmentsSource } from "./environments";
import { createGitSlice, gitScopeKey } from "./git";
import type { AppState } from "./types";

const makeStore = () =>
  create<AppState>()(
    (...a) => ({ ...createGitSlice(...a), setLastError: vi.fn() }) as unknown as AppState,
  );

/** A promise the test resolves by hand. */
const deferred = <T>() => {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
};

const local = gitScopeKey(LOCAL_ENVIRONMENT_ID, "a1");
const host = gitScopeKey("host-1", "a1");

beforeEach(() =>
  setEnvironmentsSource(() => ({ activeEnvironmentId: LOCAL_ENVIRONMENT_ID, environments: {} })),
);

describe("runGitBusy", () => {
  it("holds the verb for the checkout while the action runs, then lets go", async () => {
    const store = makeStore();
    const push = deferred<string>();
    const run = store.getState().runGitBusy(local, "Pushing…", () => push.promise);
    expect(store.getState().gitBusy[local]).toBe("Pushing…");

    push.resolve("pushed");
    expect(await run).toBe("pushed");
    expect(store.getState().gitBusy).toEqual({});
  });

  it("is exclusive: a second action on a busy checkout does nothing", async () => {
    const store = makeStore();
    const first = deferred<void>();
    const second = vi.fn(async () => "ran");
    const held = store.getState().runGitBusy(local, "Pushing…", () => first.promise);

    expect(await store.getState().runGitBusy(local, "Merging…", second)).toBeUndefined();
    expect(second).not.toHaveBeenCalled();
    // Still the first action's lock, with the first action's verb.
    expect(store.getState().gitBusy[local]).toBe("Pushing…");

    first.resolve();
    await held;
    expect(store.getState().gitBusy).toEqual({});
    // Free again: the refused action can be asked for once more.
    expect(await store.getState().runGitBusy(local, "Merging…", second)).toBe("ran");
  });

  it("keeps the same checkout on two hosts apart, and releases only its own", async () => {
    const store = makeStore();
    const a = deferred<void>();
    const b = deferred<void>();
    // Hosts reuse agent ids: the push on this Mac is not host-1's.
    const onLocal = store.getState().runGitBusy(local, "Pushing…", () => a.promise);
    const onHost = store.getState().runGitBusy(host, "Rebasing…", () => b.promise);
    expect(store.getState().gitBusy).toEqual({ [local]: "Pushing…", [host]: "Rebasing…" });

    a.resolve();
    await onLocal;
    expect(store.getState().gitBusy).toEqual({ [host]: "Rebasing…" });
    b.resolve();
    await onHost;
    expect(store.getState().gitBusy).toEqual({});
  });

  it("releases on failure too", async () => {
    const store = makeStore();
    await expect(
      store.getState().runGitBusy(local, "Pushing…", async () => {
        throw new Error("boom");
      }),
    ).rejects.toThrow("boom");
    expect(store.getState().gitBusy).toEqual({});
  });
});

describe("noteGitAction", () => {
  it("posts the outcome under the key the action started with, not the host on screen", () => {
    const store = makeStore();
    // The panel on this Mac began the push and kept its scoped key; the user
    // has since switched to host-1. The confirmation is still this Mac's.
    setEnvironmentsSource(() => ({ activeEnvironmentId: "host-1", environments: {} }));
    store.getState().noteGitAction(local, "Pushed to origin");
    expect(store.getState().gitNotices).toEqual({ [local]: "Pushed to origin" });
  });
});
