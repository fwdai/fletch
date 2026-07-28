import { beforeEach, describe, expect, it } from "vitest";

import { acceptPrWrite, issuePrWrite, resetPrWriteOrder } from "./prWriteOrder";

describe("prWriteOrder", () => {
  beforeEach(() => resetPrWriteOrder());

  /** The core guarantee: when two polls for the same agent are in flight and the
   *  older one resolves last, its write is dropped rather than clobbering the
   *  newer state. This is the merged-flips-back-to-open regression. */
  it("drops an older-issued write that lands after a newer one", () => {
    const stale = issuePrWrite(); // Git panel poll, issued first
    const fresh = issuePrWrite(); // title capsule poll, issued second

    // The newer one resolves first and lands.
    expect(acceptPrWrite("prStates", "agent-1", fresh)).toBe(true);
    // The older one resolves last and must be ignored.
    expect(acceptPrWrite("prStates", "agent-1", stale)).toBe(false);
  });

  /** In-order completion is the common case and must always apply. */
  it("accepts writes that land in issue order", () => {
    const first = issuePrWrite();
    const second = issuePrWrite();
    expect(acceptPrWrite("prStates", "agent-1", first)).toBe(true);
    expect(acceptPrWrite("prStates", "agent-1", second)).toBe(true);
  });

  /** The post-merge refresh is issued after any in-flight poll, so it wins even
   *  if that poll (which would report the PR still open) resolves later. */
  it("lets a post-merge refresh survive a poll that resolves after it", () => {
    const poll = issuePrWrite(); // issued before the merge
    const postMerge = issuePrWrite(); // mergePr's refresh

    expect(acceptPrWrite("prStates", "agent-1", postMerge)).toBe(true);
    expect(acceptPrWrite("prStates", "agent-1", poll)).toBe(false);
  });

  /** Tickets are scoped per slice: a review-threads write must not block a
   *  checks write for the same agent, since they're independent slices. */
  it("scopes ordering per slice", () => {
    const checks = issuePrWrite();
    const comments = issuePrWrite();

    expect(acceptPrWrite("prComments", "agent-1", comments)).toBe(true);
    // Lower ticket, but a different slice — must still apply.
    expect(acceptPrWrite("prChecks", "agent-1", checks)).toBe(true);
  });

  /** And per key, so one agent's sweep entry can't block another's. */
  it("scopes ordering per key", () => {
    const a = issuePrWrite();
    const b = issuePrWrite();

    expect(acceptPrWrite("prStates", "agent-2", b)).toBe(true);
    expect(acceptPrWrite("prStates", "agent-1", a)).toBe(true);
  });

  /** A single ticket covers one (slice, key) write. Re-checking the same ticket
   *  reports false, so callers writing two slices must check each separately
   *  (which `fetchPrLive` does) rather than reusing one verdict. */
  it("treats a ticket as spent per slice and key", () => {
    const t = issuePrWrite();
    expect(acceptPrWrite("prStates", "agent-1", t)).toBe(true);
    expect(acceptPrWrite("prStates", "agent-1", t)).toBe(false);
    // Same ticket, different slice — independent counter, so it applies.
    expect(acceptPrWrite("prChecks", "agent-1", t)).toBe(true);
  });
});
