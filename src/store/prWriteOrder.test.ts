import { beforeEach, describe, expect, it } from "vitest";

import { createPrWriteOrder, type PrWriteOrder } from "./prWriteOrder";

describe("prWriteOrder", () => {
  let order: PrWriteOrder;
  beforeEach(() => {
    order = createPrWriteOrder();
  });

  /** The core guarantee: when two polls for the same agent are in flight and the
   *  older one resolves last, its write is dropped rather than clobbering the
   *  newer state. This is the merged-flips-back-to-open regression. */
  it("drops an older-issued write that lands after a newer one", () => {
    const stale = order.issue(); // Git panel poll, issued first
    const fresh = order.issue(); // title capsule poll, issued second

    // The newer one resolves first and lands.
    expect(order.accept("prStates", "agent-1", fresh)).toBe(true);
    // The older one resolves last and must be ignored.
    expect(order.accept("prStates", "agent-1", stale)).toBe(false);
  });

  /** In-order completion is the common case and must always apply. */
  it("accepts writes that land in issue order", () => {
    const first = order.issue();
    const second = order.issue();
    expect(order.accept("prStates", "agent-1", first)).toBe(true);
    expect(order.accept("prStates", "agent-1", second)).toBe(true);
  });

  /** The post-merge refresh is issued after any in-flight poll, so it wins even
   *  if that poll (which would report the PR still open) resolves later. */
  it("lets a post-merge refresh survive a poll that resolves after it", () => {
    const poll = order.issue(); // issued before the merge
    const postMerge = order.issue(); // mergePr's refresh

    expect(order.accept("prStates", "agent-1", postMerge)).toBe(true);
    expect(order.accept("prStates", "agent-1", poll)).toBe(false);
  });

  /** Tickets are scoped per slice: a review-threads write must not block a
   *  checks write for the same agent, since they're independent slices. */
  it("scopes ordering per slice", () => {
    const checks = order.issue();
    const comments = order.issue();

    expect(order.accept("prComments", "agent-1", comments)).toBe(true);
    // Lower ticket, but a different slice — must still apply.
    expect(order.accept("prChecks", "agent-1", checks)).toBe(true);
  });

  /** And per key, so one agent's sweep entry can't block another's. */
  it("scopes ordering per key", () => {
    const a = order.issue();
    const b = order.issue();

    expect(order.accept("prStates", "agent-2", b)).toBe(true);
    expect(order.accept("prStates", "agent-1", a)).toBe(true);
  });

  /** A just-created PR must not be erased by a poll that was already in flight
   *  and saw no PR at all. Without the stamp the high-water mark stays put and
   *  that poll is still accepted. */
  it("protects an authoritative write from an in-flight poll", () => {
    const poll = order.issue(); // panel poll, issued before the PR existed
    order.stamp("prStates", "agent-1"); // createPr's write lands
    expect(order.accept("prStates", "agent-1", poll)).toBe(false);
  });

  /** A backend-pushed `pr:state_changed` is likewise authoritative — a poll that
   *  observed the PR before the transition must not roll the badge back. */
  it("protects a pushed state change from an in-flight poll", () => {
    const poll = order.issue();
    order.stamp("prStates", "agent-1"); // onPrStateChanged
    expect(order.accept("prStates", "agent-1", poll)).toBe(false);
  });

  /** But a poll issued *after* an authoritative write is newer information and
   *  must still apply — otherwise the stamp would freeze the slice. */
  it("lets a later poll supersede an authoritative write", () => {
    order.stamp("prStates", "agent-1");
    const later = order.issue();
    expect(order.accept("prStates", "agent-1", later)).toBe(true);
  });

  /** Stamping one slice must not block another for the same agent. */
  it("scopes an authoritative write to its slice", () => {
    const checks = order.issue();
    order.stamp("prStates", "agent-1");
    expect(order.accept("prChecks", "agent-1", checks)).toBe(true);
  });

  /** A fence drops every answer issued before it, including for keys nothing
   *  ever stamped — the answers of a host the store has since left. */
  it("drops every write issued before a fence", () => {
    const before = order.issue();
    order.fence();
    expect(order.accept("prStates", "agent-1", before)).toBe(false);
    expect(order.accept("prComments", "agent-2", before)).toBe(false);
  });

  /** …and none issued after it, whatever the old host stamped. */
  it("lets a write issued after a fence apply over the old host's stamps", () => {
    order.stamp("prStates", "agent-1");
    order.fence();
    const fresh = order.issue();
    expect(order.accept("prStates", "agent-1", fresh)).toBe(true);
  });

  /** A single ticket covers one (slice, key) write. Re-checking the same ticket
   *  reports false, so callers writing two slices must check each separately
   *  (which `fetchPrLive` does) rather than reusing one verdict. */
  it("treats a ticket as spent per slice and key", () => {
    const t = order.issue();
    expect(order.accept("prStates", "agent-1", t)).toBe(true);
    expect(order.accept("prStates", "agent-1", t)).toBe(false);
    // Same ticket, different slice — independent counter, so it applies.
    expect(order.accept("prChecks", "agent-1", t)).toBe(true);
  });
});
