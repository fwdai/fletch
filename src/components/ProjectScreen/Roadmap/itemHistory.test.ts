import { describe, expect, it } from "vitest";
import type { RoadmapItemEvent } from "@/api";
import { eventDetailUrl, eventLine, insertEvent, mergeSnapshot } from "./itemHistory";

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

  it("labels a hand-built item's opening line", () => {
    // `created` is the user-typed row's `proposed`. The label map is typed
    // `Record<RoadmapEventKind, …>`, so omitting it would fail `tsc` — this only
    // pins the wording the card shows.
    expect(eventLine(event({ id: "c", kind: "created", actor: "user" }))).toBe("Created");
  });
});

describe("eventDetailUrl", () => {
  it("recognizes a detail that is nothing but a link — the pr_opened line", () => {
    const url = "https://github.com/o/r/pull/42";
    expect(eventDetailUrl(event({ id: "a", kind: "pr_opened", detail: url }))).toBe(url);
    // Whitespace around it is still just a link.
    expect(eventDetailUrl(event({ id: "b", kind: "pr_opened", detail: ` ${url} ` }))).toBe(url);
  });

  it("leaves prose alone, however link-like", () => {
    // A reason that mentions a URL is a sentence, not an address.
    for (const detail of [
      null,
      "",
      "its run failed",
      "opened https://github.com/o/r/pull/42",
      // Not https: nothing on this trail should open a non-https scheme.
      "http://github.com/o/r/pull/42",
      "file:///etc/passwd",
    ]) {
      expect(eventDetailUrl(event({ id: "c", detail }))).toBeNull();
    }
  });
});
