import { describe, expect, it } from "vitest";
import type { RoadmapItem, RoadmapItemEvent, RoadmapProjectHold } from "@/api";
import { buildNeedsYou, latestByItem, mergeLatest, upsertLatest } from "./select";

// ── fixtures ──────────────────────────────────────────────────────────────────

function item(over: Partial<RoadmapItem> & { id: string }): RoadmapItem {
  return {
    project_id: "p1",
    code: `MCA-${over.id}`,
    title: `Item ${over.id}`,
    why: "",
    horizon: "next",
    status: "active",
    rank: 1,
    area: null,
    source: "user",
    accept: [],
    deps: [],
    agent_id: null,
    workflow_def_id: null,
    run_id: null,
    pr_url: null,
    pr_number: null,
    hold_reason: null,
    held_by: null,
    held_at: null,
    close_reason: null,
    issue_url: null,
    created_at: 0,
    updated_at: 0,
    ...over,
  };
}

/** A held item — the trio, as the backend writes it (all three together). */
function held(id: string, reason: string, over: Partial<RoadmapItem> = {}): RoadmapItem {
  return item({
    id,
    hold_reason: reason,
    held_by: "pm",
    held_at: 100,
    ...over,
  });
}

function projectHold(over: Partial<RoadmapProjectHold> = {}): RoadmapProjectHold {
  return {
    project_id: "p1",
    reason: "re-planning the quarter",
    held_by: "pm",
    created_at: 50,
    ...over,
  };
}

function event(
  over: Partial<RoadmapItemEvent> & { id: string; item_id: string },
): RoadmapItemEvent {
  return {
    project_id: "p1",
    actor: "drainer",
    kind: "blocked",
    detail: null,
    created_at: 1,
    ...over,
  };
}

/** The default shape: one item per case, no events. */
function input(over: Partial<Parameters<typeof buildNeedsYou>[0]> = {}) {
  return { items: [], latestEvents: [], ...over };
}

// ── the durable board wedge ───────────────────────────────────────────────────

describe("buildNeedsYou — blocked items", () => {
  it("cards a queued item whose newest event is blocked, naming the cycle", () => {
    const cards = buildNeedsYou(
      input({
        items: [item({ id: "a", status: "queued" })],
        latestEvents: [event({ id: "e", item_id: "a", detail: "MCA-101 → MCA-104 → MCA-101" })],
      }),
    );
    expect(cards).toHaveLength(1);
    expect(cards[0]).toMatchObject({
      id: "blocked:a",
      reason: "item-blocked",
      detail: "MCA-101 → MCA-104 → MCA-101",
    });
  });

  it("drops it once the item leaves the queue — nothing is waiting to dispatch", () => {
    for (const status of ["open", "active", "in_review"] as const) {
      const cards = buildNeedsYou(
        input({
          items: [item({ id: "a", status })],
          latestEvents: [event({ id: "e", item_id: "a" })],
        }),
      );
      expect(cards, status).toEqual([]);
    }
  });

  it("cards every standing wedge, not just the dependency cycle", () => {
    // `blocked` is written for anything that will not resolve without a person:
    // a cycle, a queued item with no workflow, an invalid spec, a project with no
    // repo. They differ only in the detail, and each is a decision on this strip.
    for (const detail of [
      "Stuck in a dependency loop: MCA-101 → MCA-104 → MCA-101",
      "No workflow to run it under. Pick one on this item, or set the project's default workflow.",
      "Its workflow is missing or no longer valid — pick another.",
      "This project has no repo to run in.",
    ]) {
      const cards = buildNeedsYou(
        input({
          items: [item({ id: "a", status: "queued" })],
          latestEvents: [event({ id: "e", item_id: "a", detail })],
        }),
      );
      expect(
        cards.map((c) => c.reason),
        detail,
      ).toEqual(["item-blocked"]);
      expect(cards[0].detail).toBe(detail);
    }
  });

  it("never cards an ending — those are records, not decisions", () => {
    // A run that failed, was cancelled or was deleted leaves the item `open`, and
    // a closed PR puts it back on the board: the user can act on the row itself.
    // A card for each would be a notification feed.
    for (const kind of ["run_failed", "run_canceled", "run_deleted", "pr_closed"] as const) {
      const cards = buildNeedsYou(
        input({
          items: [item({ id: "a", status: "queued" })],
          latestEvents: [event({ id: "e", item_id: "a", kind })],
        }),
      );
      expect(cards, kind).toEqual([]);
    }
  });

  it("drops it once the trail moves on, even with the block still in the list", () => {
    const cards = buildNeedsYou(
      input({
        items: [item({ id: "a", status: "queued" })],
        latestEvents: [
          event({ id: "old", item_id: "a", created_at: 5 }),
          event({ id: "new", item_id: "a", kind: "dispatched", created_at: 6 }),
        ],
      }),
    );
    expect(cards).toEqual([]);
  });
});

// ── the brake ─────────────────────────────────────────────────────────────────

describe("buildNeedsYou — holds", () => {
  it("cards a held item at any on-board status, quoting the reason verbatim", () => {
    // Read off the row, not off the trail: a hold is a current fact, so there is
    // no "has the trail moved on" question — and it applies wherever the item is,
    // because the PM can pull the brake mid-run.
    for (const status of ["proposed", "open", "queued", "active", "in_review"] as const) {
      const cards = buildNeedsYou(
        input({ items: [held("a", "confirm the scope first", { status })] }),
      );
      expect(cards, status).toHaveLength(1);
      expect(cards[0]).toMatchObject({
        id: "held:a",
        reason: "item-held",
        itemId: "a",
        code: "MCA-a",
        detail: "confirm the scope first",
        // The hold's own timestamp, so editing a held item doesn't refloat it.
        activityAt: 100,
      });
    }
  });

  it("drops the card the moment the hold is lifted", () => {
    expect(buildNeedsYou(input({ items: [item({ id: "a", status: "queued" })] }))).toEqual([]);
  });

  it("cards the board's hold with no item to jump to", () => {
    const cards = buildNeedsYou(input({ projectHold: projectHold() }));
    expect(cards).toHaveLength(1);
    expect(cards[0]).toMatchObject({
      id: "project-held:p1",
      reason: "project-held",
      detail: "re-planning the quarter",
      activityAt: 50,
    });
    // The one card that names no row: there is nothing to focus, and the strip
    // renders a label instead of a button.
    expect(cards[0].itemId).toBeUndefined();
    expect(cards[0].code).toBeUndefined();
  });

  it("cards both scopes at once, board first", () => {
    // Two separate decisions: lifting the board's hold does not lift the item's,
    // so both have to be visible and both have to be releasable.
    const cards = buildNeedsYou(
      input({ items: [held("a", "wrong scope")], projectHold: projectHold() }),
    );
    expect(cards.map((c) => c.id)).toEqual(["project-held:p1", "held:a"]);
  });

  it("falls back to the row's updated_at when a hold predates held_at", () => {
    // Defensive: the trio is written together, so this is only reachable through
    // a hand-edited row — but ordering must not turn into NaN if it happens.
    const cards = buildNeedsYou(
      input({ items: [held("a", "why", { held_at: null, updated_at: 7 })] }),
    );
    expect(cards[0].activityAt).toBe(7);
  });
});

// ── the join ──────────────────────────────────────────────────────────────────

describe("buildNeedsYou — join misses", () => {
  it("skips an event whose item isn't on this board", () => {
    const cards = buildNeedsYou(
      input({ items: [item({ id: "a" })], latestEvents: [event({ id: "e", item_id: "zzz" })] }),
    );
    expect(cards).toEqual([]);
  });
});

// ── ordering ──────────────────────────────────────────────────────────────────

describe("buildNeedsYou — ordering", () => {
  it("is most-decidable-first across the reasons", () => {
    const cards = buildNeedsYou(
      input({
        items: [item({ id: "e", status: "queued" }), held("f", "wrong scope")],
        latestEvents: [event({ id: "ev", item_id: "e" })],
        projectHold: projectHold(),
      }),
    );
    // The two holds sit above the wedge: Release is one click, and the board's
    // hold stops the most.
    expect(cards.map((c) => c.reason)).toEqual(["project-held", "item-held", "item-blocked"]);
  });

  it("breaks a bucket tie on recency, then on id", () => {
    const cards = buildNeedsYou(
      input({
        items: [
          held("old", "why", { held_at: 10 }),
          held("new", "why", { held_at: 20 }),
          // Same instant as `new`: the id decides, so the order never flickers.
          held("also", "why", { held_at: 20 }),
        ],
      }),
    );
    expect(cards.map((c) => c.itemId)).toEqual(["also", "new", "old"]);
  });
});

// ── the latest-event helpers ──────────────────────────────────────────────────

describe("latest-per-item folding", () => {
  it("keeps the newest row per item", () => {
    const latest = latestByItem([
      event({ id: "a1", item_id: "a", created_at: 1 }),
      event({ id: "a2", item_id: "a", kind: "dispatched", created_at: 2 }),
      event({ id: "b1", item_id: "b", created_at: 9 }),
    ]);
    expect(latest.get("a")?.id).toBe("a2");
    expect(latest.get("b")?.id).toBe("b1");
  });

  it("upserts a newer event and ignores an older one, by reference", () => {
    const held = [event({ id: "a2", item_id: "a", created_at: 5 })];
    expect(upsertLatest(held, event({ id: "a1", item_id: "a", created_at: 1 }))).toBe(held);
    expect(upsertLatest(held, event({ id: "a2", item_id: "a", created_at: 5 }))).toBe(held);

    const newer = upsertLatest(held, event({ id: "a3", item_id: "a", created_at: 6 }));
    expect(newer.map((e) => e.id)).toEqual(["a3"]);
    expect(upsertLatest(held, event({ id: "b1", item_id: "b" })).map((e) => e.id)).toEqual([
      "a2",
      "b1",
    ]);
  });

  it("does not let a snapshot clobber a block that landed while it was in flight", () => {
    // The load's race: the drainer wedges an item after the backend read its
    // rows. Merging (not replacing) is what keeps the card.
    const live = [event({ id: "live", item_id: "a", created_at: 9 })];
    const merged = mergeLatest(live, [
      event({ id: "stale", item_id: "a", kind: "queued", created_at: 3 }),
      event({ id: "other", item_id: "b", kind: "queued", created_at: 4 }),
    ]);
    expect(merged.map((e) => e.id)).toEqual(["live", "other"]);
  });
});
