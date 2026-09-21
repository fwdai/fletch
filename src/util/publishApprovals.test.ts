// The replay boundary, as a function of what arrived and when. The two cases
// that matter are the ones an unguarded wholesale replace gets wrong: a request
// raised after the host read its queue (dropped, and the host stays blocked
// with nothing on screen), and a resolution that beat the answer back (put
// back, so a card offers to answer a question that is over).

import { describe, expect, it } from "vitest";
import type { PublishApproval } from "@/api";
import { replayApprovalEvents } from "./publishApprovals";

const req = (id: string): PublishApproval => ({
  id,
  agent_id: "a1",
  op: "git_push",
  detail: `push ${id}`,
});

const ids = (rows: PublishApproval[]) => rows.map((r) => r.id);

describe("replayApprovalEvents", () => {
  it("keeps the snapshot when nothing raced it", () => {
    const snapshot = [req("r1")];
    expect(replayApprovalEvents(snapshot, [])).toBe(snapshot);
  });

  it("keeps a request raised after the queue was read", () => {
    expect(
      ids(replayApprovalEvents([req("r1")], [{ kind: "requested", request: req("r2") }])),
    ).toEqual(["r1", "r2"]);
  });

  it("does not double a request the snapshot already carries", () => {
    // The event beat the response, and the host had already counted it: one
    // card, not two.
    const events = [{ kind: "requested", request: req("r1") } as const];
    expect(ids(replayApprovalEvents([req("r1")], events))).toEqual(["r1"]);
  });

  it("drops a request the host has since resolved", () => {
    const events = [{ kind: "resolved", id: "r1" } as const];
    expect(ids(replayApprovalEvents([req("r1"), req("r2")], events))).toEqual(["r2"]);
  });

  it("drops one raised and resolved entirely within the window", () => {
    const events = [
      { kind: "requested", request: req("r2") } as const,
      { kind: "resolved", id: "r2" } as const,
    ];
    expect(ids(replayApprovalEvents([req("r1")], events))).toEqual(["r1"]);
  });

  it("ignores a resolution for something no one is holding", () => {
    const events = [{ kind: "resolved", id: "never-seen" } as const];
    expect(ids(replayApprovalEvents([req("r1")], events))).toEqual(["r1"]);
  });

  it("leaves the snapshot array untouched", () => {
    const snapshot = [req("r1")];
    replayApprovalEvents(snapshot, [
      { kind: "requested", request: req("r2") },
      { kind: "resolved", id: "r1" },
    ]);
    expect(ids(snapshot)).toEqual(["r1"]);
  });
});
