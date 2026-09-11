import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  openProviderLogin,
  writeProviderLogin,
  resizeProviderLogin,
  closeProviderLogin,
  onProviderLoginOutput,
  onProviderLoginExit,
} = vi.hoisted(() => ({
  openProviderLogin: vi.fn(),
  writeProviderLogin: vi.fn(),
  resizeProviderLogin: vi.fn(),
  closeProviderLogin: vi.fn(),
  onProviderLoginOutput: vi.fn(),
  onProviderLoginExit: vi.fn(),
}));
vi.mock("@/api", () => ({
  api: { openProviderLogin, writeProviderLogin, resizeProviderLogin, closeProviderLogin },
  onProviderLoginOutput,
  onProviderLoginExit,
}));

import type { ProviderLoginExitEvent, ProviderLoginOutputEvent } from "@/api";

const ID = "claude";

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (err: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (err: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Let every already-settled promise chain run to completion. */
const flush = () => new Promise((r) => setTimeout(r, 0));

type Sessions = typeof import("./loginSessions");

describe("loginSessions", () => {
  let sessions: Sessions;
  /** The pending `listen(...)` registrations — a real tap only exists once
   *  these resolve, which is the whole point of the ordering under test. */
  let outputTap: Deferred<() => void>;
  let exitTap: Deferred<() => void>;
  /** The handlers the module passed to `listen`, i.e. the backend's side. */
  let emitOutput: ((e: ProviderLoginOutputEvent) => void) | undefined;
  let emitExit: ((e: ProviderLoginExitEvent) => void) | undefined;
  /** Backend calls in the order they were made. */
  let calls: string[];

  const registerTaps = () => {
    outputTap.resolve(() => {});
    exitTap.resolve(() => {});
  };

  beforeEach(async () => {
    vi.resetModules(); // the module holds per-session state at module level
    vi.clearAllMocks();
    calls = [];
    emitOutput = undefined;
    emitExit = undefined;
    outputTap = deferred();
    exitTap = deferred();
    // Tauri's `listen` only attaches the handler when its promise resolves;
    // until then the backend has nowhere to deliver an event, so the mocks
    // publish their emitter at exactly that moment.
    onProviderLoginOutput.mockImplementation((cb: (e: ProviderLoginOutputEvent) => void) =>
      outputTap.promise.then((unlisten) => {
        emitOutput = cb;
        return unlisten;
      }),
    );
    onProviderLoginExit.mockImplementation((cb: (e: ProviderLoginExitEvent) => void) =>
      exitTap.promise.then((unlisten) => {
        emitExit = cb;
        return unlisten;
      }),
    );
    openProviderLogin.mockImplementation(() => {
      calls.push("open");
      return Promise.resolve();
    });
    writeProviderLogin.mockImplementation(() => {
      calls.push("write");
      return Promise.resolve();
    });
    closeProviderLogin.mockResolvedValue(undefined);
    sessions = await import("./loginSessions");
  });

  it("does not open the PTY until both event taps are registered", async () => {
    const run = sessions.runLogin(ID, 80, 24);
    await flush();
    expect(openProviderLogin).not.toHaveBeenCalled();

    outputTap.resolve(() => {});
    await flush();
    expect(openProviderLogin).not.toHaveBeenCalled(); // the exit tap is still pending

    exitTap.resolve(() => {});
    await run;
    expect(openProviderLogin).toHaveBeenCalledWith(ID, 80, 24);
  });

  it("applies output and an exit that arrive the instant the PTY opens", async () => {
    openProviderLogin.mockImplementation(() => {
      // A CLI that is already signed in prints and exits immediately: these
      // land before the caller of `openProviderLogin` is resumed.
      emitOutput?.({ id: ID, bytes: btoa("already signed in\r\n") });
      emitExit?.({ id: ID, success: true, message: "" });
      return Promise.resolve();
    });
    const seen: (ProviderLoginExitEvent | undefined)[] = [];
    sessions.subscribeLoginExit(ID, (e) => seen.push(e));

    registerTaps();
    await sessions.runLogin(ID, 80, 24);

    expect(sessions.getLoginExit(ID)).toEqual({ id: ID, success: true, message: "" });
    expect(seen).toContainEqual({ id: ID, success: true, message: "" });
    expect(new TextDecoder().decode(sessions.readLoginBuffer(ID))).toBe("already signed in\r\n");
  });

  it("reports a failed tap registration as the session outcome and retries next run", async () => {
    outputTap.reject(new Error("listen failed"));
    exitTap.resolve(() => {});

    await sessions.runLogin(ID, 80, 24);

    expect(openProviderLogin).not.toHaveBeenCalled();
    expect(sessions.getLoginExit(ID)).toEqual({
      id: ID,
      success: false,
      message: "Error: listen failed",
    });
    expect(onProviderLoginOutput).toHaveBeenCalledTimes(1);

    // "Run again" re-registers rather than inheriting the rejected promise.
    outputTap = deferred();
    exitTap = deferred();
    registerTaps();
    await sessions.runLogin(ID, 80, 24);

    expect(onProviderLoginOutput).toHaveBeenCalledTimes(2);
    expect(onProviderLoginExit).toHaveBeenCalledTimes(2);
    expect(openProviderLogin).toHaveBeenCalledTimes(1);
    expect(sessions.getLoginExit(ID)).toBeUndefined();
  });

  it("queues a keystroke behind the whole open, tap registration included", async () => {
    sessions.runLogin(ID, 80, 24);
    const write = sessions.writeLogin(ID, "y\r");
    await flush();
    expect(calls).toEqual([]);

    registerTaps();
    await write;
    expect(calls).toEqual(["open", "write"]);
  });

  it("re-attaching does not reopen a sign-in this session already started", async () => {
    registerTaps();
    await sessions.runLogin(ID, 80, 24);
    sessions.attachLogin(ID, 100, 30);
    await flush();

    expect(openProviderLogin).toHaveBeenCalledTimes(1);
  });
});
