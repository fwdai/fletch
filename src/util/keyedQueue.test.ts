// The per-key op chain behind session-config writes and project-switch writes.
// What matters is the contract both rely on: same key runs in issue order, a
// failure neither blocks the chain nor hides from its own caller, and different
// keys don't wait on each other.

import { describe, expect, it, vi } from "vitest";
import { createKeyedQueue } from "./keyedQueue";

const deferred = <T = void>() => {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
};

describe("createKeyedQueue", () => {
  it("runs ops for one key in issue order, each starting after the previous settles", async () => {
    const q = createKeyedQueue();
    const first = deferred();
    const started: string[] = [];

    const a = q.run("k", () => {
      started.push("a");
      return first.promise;
    });
    const b = q.run("k", async () => {
      started.push("b");
    });
    // `a` starts once the (empty) chain ahead of it settles; `b` must not start
    // until `a` does, however long that takes.
    await vi.waitFor(() => expect(started).toEqual(["a"]));

    first.resolve();
    await Promise.all([a, b]);
    expect(started).toEqual(["a", "b"]);
  });

  it("does not let one key's op hold up another key's", async () => {
    const q = createKeyedQueue();
    const stuck = deferred();
    const ran: string[] = [];

    void q.run("slow", () => stuck.promise);
    await q.run("fast", async () => {
      ran.push("fast");
    });
    expect(ran).toEqual(["fast"]);
    stuck.resolve();
  });

  it("surfaces a failure to its caller but lets the next op run", async () => {
    const q = createKeyedQueue();
    const failed = q.run("k", () => Promise.reject(new Error("boom")));
    const next = q.run("k", async () => "ok");

    await expect(failed).rejects.toThrow("boom");
    await expect(next).resolves.toBe("ok");
  });

  it("exposes the pending tail for a key, and clears it once settled", async () => {
    const q = createKeyedQueue();
    expect(q.pending("k")).toBeUndefined();

    const gate = deferred();
    const op = q.run("k", () => gate.promise);
    expect(q.pending("k")).toBeDefined();

    gate.resolve();
    await op;
    // The finally that clears the tail runs a microtask after `op` resolves.
    await Promise.resolve();
    expect(q.pending("k")).toBeUndefined();
  });

  it("keeps the tail pointed at the newest op when an older one settles", async () => {
    // Clearing on settle must only clear if nothing newer was queued since —
    // otherwise a dependent action would see "nothing pending" while a later
    // write is still in flight.
    const q = createKeyedQueue();
    const first = deferred();
    const second = deferred();
    const a = q.run("k", () => first.promise);
    void q.run("k", () => second.promise);

    first.resolve();
    await a;
    await Promise.resolve();
    expect(q.pending("k")).toBeDefined();
    second.resolve();
  });
});
