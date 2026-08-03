import { describe, expect, it } from "vitest";
import type { RoadmapItemEvent } from "@/api";
import { eventLine, insertEvent, mergeSnapshot } from "./itemHistory";

function event(over: Partial<RoadmapItemEvent> & { id: string }): RoadmapItemEvent {
  return {
    item_id: "i1",
    project_id: "p1",
    actor: "drainer",
    kind: "dispatched",
    detail: null,
    created_at: 0,
    ...over,
  };
}

describe("insertEvent", () => {
  it("prepends a live event, newest first", () => {
    const older = event({ id: "a", created_at: 10 });
    const newer = event({ id: "b", created_at: 20 });
    expect(insertEvent([older], newer)).toEqual([newer, older]);
  });

  it("drops a duplicate id — the same event can arrive live and in a snapshot", () => {
    const e = event({ id: "a", created_at: 10 });
    const trail = insertEvent([], e);
    expect(insertEvent(trail, e)).toBe(trail);
  });
});

describe("mergeSnapshot", () => {
  it("lays the snapshot under live arrivals without duplicating the overlap", () => {
    // The lazy-load race: `c` landed while the fetch was in flight, so it is in
    // both the live buffer and the snapshot. It must appear once, in order.
    const live = [event({ id: "c", created_at: 30 })];
    const snapshot = [
      event({ id: "c", created_at: 30 }),
      event({ id: "b", created_at: 20 }),
      event({ id: "a", created_at: 10 }),
    ];
    expect(mergeSnapshot(live, snapshot).map((e) => e.id)).toEqual(["c", "b", "a"]);
  });

  it("keeps the trail newest first when live events outrun the snapshot", () => {
    const live = [event({ id: "d", created_at: 40 })];
    const snapshot = [event({ id: "a", created_at: 10 })];
    expect(mergeSnapshot(live, snapshot).map((e) => e.id)).toEqual(["d", "a"]);
  });
});

describe("eventLine", () => {
  it("says the kind, with the detail when there is one", () => {
    expect(eventLine(event({ id: "a", kind: "run_failed", detail: "its run failed" }))).toBe(
      "Run failed — its run failed",
    );
    expect(eventLine(event({ id: "b", kind: "shipped" }))).toBe("Shipped");
  });
});
