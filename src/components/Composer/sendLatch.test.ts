import { describe, expect, it } from "vitest";
import { createSendLatch } from "./sendLatch";

/** A promise settled from outside, standing in for a `list_dir` still out. */
function deferred() {
  let resolve!: () => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const settle = () => new Promise((r) => setTimeout(r, 0));

describe("createSendLatch", () => {
  it("refuses a second send while the first is still resolving folders", async () => {
    const latch = createSendLatch();
    const lookup = deferred();
    const sent: string[] = [];

    expect(
      latch.run(async () => {
        await lookup.promise;
        sent.push("first");
      }),
    ).toBe(true);
    expect(latch.busy()).toBe(true);

    // The second send arrives mid-lookup: refused, never run.
    let secondRan = false;
    expect(
      latch.run(async () => {
        secondRan = true;
      }),
    ).toBe(false);

    lookup.resolve();
    await settle();
    expect(sent).toEqual(["first"]);
    expect(secondRan).toBe(false);
    expect(latch.busy()).toBe(false);

    // Once the first has gone out, the next send runs.
    expect(
      latch.run(async () => {
        sent.push("second");
      }),
    ).toBe(true);
    await settle();
    expect(sent).toEqual(["first", "second"]);
  });

  it("lets go when the send fails", async () => {
    const latch = createSendLatch();
    const lookup = deferred();
    latch.run(() => lookup.promise);
    lookup.reject(new Error("list_dir failed"));
    await settle();
    expect(latch.busy()).toBe(false);
  });
});
