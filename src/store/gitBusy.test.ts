// The Git panel's busy lock: one action per checkout per host, held in the
// store so it outlives the panel section that started it (the section mounts
// per checkout), and released only by its holder.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

vi.mock("@/api", () => ({ api: {} }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

import { type EnvironmentEntry, LOCAL_ENVIRONMENT_ID, setEnvironmentsSource } from "./environments";
import { createGitSlice, gitScopeKey } from "./git";
import type { AppState } from "./types";

const host: EnvironmentEntry = {
  id: "host-1",
  name: "Cloud box",
  kind: "remote",
  connection: "connected",
};
const activeIs = (entry?: EnvironmentEntry) =>
  setEnvironmentsSource(() => ({
    activeEnvironmentId: entry?.id ?? LOCAL_ENVIRONMENT_ID,
    environments: entry ? { [entry.id]: entry } : {},
  }));

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

beforeEach(() => activeIs());

describe("runGitBusy", () => {
  it("holds the verb for the checkout while the action runs, then lets go", async () => {
    const store = makeStore();
    const push = deferred<string>();
    const run = store.getState().runGitBusy("a1", "Pushing…", () => push.promise);
    expect(store.getState().gitBusy[gitScopeKey(LOCAL_ENVIRONMENT_ID, "a1")]).toBe("Pushing…");

    push.resolve("pushed");
    expect(await run).toBe("pushed");
    expect(store.getState().gitBusy).toEqual({});
  });

  it("is exclusive: a second action on a busy checkout does nothing", async () => {
    const store = makeStore();
    const first = deferred<void>();
    const second = vi.fn(async () => "ran");
    const held = store.getState().runGitBusy("a1", "Pushing…", () => first.promise);

    expect(await store.getState().runGitBusy("a1", "Merging…", second)).toBeUndefined();
    expect(second).not.toHaveBeenCalled();
    // Still the first action's lock, with the first action's verb.
    expect(store.getState().gitBusy[gitScopeKey(LOCAL_ENVIRONMENT_ID, "a1")]).toBe("Pushing…");

    first.resolve();
    await held;
    expect(store.getState().gitBusy).toEqual({});
    // Free again: the refused action can be asked for once more.
    expect(await store.getState().runGitBusy("a1", "Merging…", second)).toBe("ran");
  });

  it("locks checkouts apart, and the same checkout on another host apart", async () => {
    const store = makeStore();
    const a = deferred<void>();
    const b = deferred<void>();
    const onLocal = store.getState().runGitBusy("a1", "Pushing…", () => a.promise);
    // Hosts reuse agent ids: the push on this Mac is not host-1's.
    activeIs(host);
    const onHost = store.getState().runGitBusy("a1", "Rebasing…", () => b.promise);
    expect(store.getState().gitBusy).toEqual({
      [gitScopeKey(LOCAL_ENVIRONMENT_ID, "a1")]: "Pushing…",
      [gitScopeKey("host-1", "a1")]: "Rebasing…",
    });

    // The local push finishing while the host is on screen releases only its
    // own lock, not the host's.
    a.resolve();
    await onLocal;
    expect(store.getState().gitBusy).toEqual({ [gitScopeKey("host-1", "a1")]: "Rebasing…" });
    b.resolve();
    await onHost;
    expect(store.getState().gitBusy).toEqual({});
  });

  it("releases on failure too", async () => {
    const store = makeStore();
    await expect(
      store.getState().runGitBusy("a1", "Pushing…", async () => {
        throw new Error("boom");
      }),
    ).rejects.toThrow("boom");
    expect(store.getState().gitBusy).toEqual({});
  });
});
