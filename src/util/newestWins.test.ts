// The ordering rule, as a function of when claims were taken. The case it
// exists for is the last one: the older response arriving last, which is where
// a blind write puts a stale snapshot on screen.

import { describe, expect, it } from "vitest";
import { newestWins } from "./newestWins";

describe("newestWins", () => {
  it("leaves a lone claim current", () => {
    const claim = newestWins().claim();
    expect(claim.current()).toBe(true);
  });

  it("supersedes every claim but the newest", () => {
    const order = newestWins();
    const first = order.claim();
    const second = order.claim();
    const third = order.claim();

    expect(first.current()).toBe(false);
    expect(second.current()).toBe(false);
    expect(third.current()).toBe(true);
  });

  it("does not revive an older claim when the newest is done with", () => {
    // `current()` is a standing question, not a one-shot: an older response
    // that lands after the newest has already applied is still the stale one.
    const order = newestWins();
    const older = order.claim();
    const newer = order.claim();

    expect(newer.current()).toBe(true);
    expect(older.current()).toBe(false);
    expect(older.current()).toBe(false);
  });

  it("keeps separate orders apart", () => {
    // One order per thing refetched: a workspace refresh must not supersede an
    // approvals load.
    const a = newestWins();
    const b = newestWins();
    const claim = a.claim();
    b.claim();

    expect(claim.current()).toBe(true);
  });
});
