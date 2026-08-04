import { describe, expect, it } from "vitest";
import type {
  RoadmapItem,
  RoadmapItemEvent,
  RoadmapProjectHold,
  WfPausedReason,
  WfRun,
} from "@/api";
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

function run(over: Partial<WfRun> & { id: string }): WfRun {
  return {
    definition_id: null,
    parent_run_id: null,
    name: `run-${over.id}`,
    spec: null,
    task: "",
    project_id: "p1",
    repo_path: "/repo",
    run_dir: "/runs/x",
    branch: "wf/x",
    base_sha: "abc",
    status: "paused",
    paused_reason: "question",
    cursor: null,
    budgets: null,
    spent: null,
    error: null,
    pr_number: null,
    pr_url: null,
    roadmap_item_id: null,
    created_at: 0,
    updated_at: 1,
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

/** The default shape: one item per case, no runs, no events. */
function input(over: Partial<Parameters<typeof buildNeedsYou>[0]> = {}) {
  return { items: [], runs: [], latestEvents: [], ...over };
}

// ── one card per reason ───────────────────────────────────────────────────────

describe("buildNeedsYou — the pauses that are the user's", () => {
  const cases: { reason: WfPausedReason; card: string | null }[] = [
    { reason: "question", card: "workflow-question" },
    { reason: "approval", card: "workflow-approval" },
    { reason: "conflict", card: "workflow-conflict" },
    { reason: "budget_exceeded", card: "workflow-budget" },
    // The engine's own pauses: a gate retries, a stall is the supervisor's.
    { reason: "blocked_gate", card: null },
    { reason: "stalled", card: null },
  ];

  for (const { reason, card } of cases) {
    it(`${reason} → ${card ?? "no card"}`, () => {
      const cards = buildNeedsYou(
        input({
          items: [item({ id: "a" })],
          runs: [run({ id: "r", paused_reason: reason, roadmap_item_id: "a" })],
        }),
      );
      expect(cards.map((c) => c.reason)).toEqual(card ? [card] : []);
    });
  }

  it("names the item and carries the run for the card's actions", () => {
    const cards = buildNeedsYou(
      input({
        items: [item({ id: "a", code: "MCA-104", title: "Ship the drainer" })],
        runs: [run({ id: "r", roadmap_item_id: "a" })],
      }),
    );
    expect(cards[0]).toMatchObject({
      id: "run:r",
      code: "MCA-104",
      title: "Ship the drainer",
      runId: "r",
      pausedReason: "question",
    });
  });

  it("ignores a run that isn't paused, and one with no roadmap item", () => {
    const cards = buildNeedsYou(
      input({
        items: [item({ id: "a" })],
        runs: [
          run({ id: "running", status: "running", paused_reason: null, roadmap_item_id: "a" }),
          run({ id: "adhoc", roadmap_item_id: null }),
        ],
      }),
    );
    expect(cards).toEqual([]);
  });
});

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
  it("skips a paused run whose item isn't on this board", () => {
    // Another project's run, or one whose item shipped since: still a real run,
    // just not a decision about this board.
    const cards = buildNeedsYou(
      input({ items: [item({ id: "a" })], runs: [run({ id: "r", roadmap_item_id: "zzz" })] }),
    );
    expect(cards).toEqual([]);
  });

  it("skips an event whose item isn't on this board", () => {
    const cards = buildNeedsYou(
      input({ items: [item({ id: "a" })], latestEvents: [event({ id: "e", item_id: "zzz" })] }),
    );
    expect(cards).toEqual([]);
  });

  it("cards an item with no run and a run with no card side by side", () => {
    const cards = buildNeedsYou(
      input({
        items: [item({ id: "a", status: "queued" }), item({ id: "b" })],
        runs: [run({ id: "r", paused_reason: "approval", roadmap_item_id: "b" })],
        latestEvents: [event({ id: "e", item_id: "a" })],
      }),
    );
    expect(cards.map((c) => c.id)).toEqual(["run:r", "blocked:a"]);
  });
});

// ── ordering ──────────────────────────────────────────────────────────────────

describe("buildNeedsYou — ordering", () => {
  it("is most-decidable-first across the reasons", () => {
    const items = ["a", "b", "c", "d", "e"].map((id) =>
      item({ id, status: id === "e" ? "queued" : "active" }),
    );
    const cards = buildNeedsYou(
      input({
        items: [...items, held("f", "wrong scope")],
        runs: [
          run({ id: "budget", paused_reason: "budget_exceeded", roadmap_item_id: "a" }),
          run({ id: "conflict", paused_reason: "conflict", roadmap_item_id: "b" }),
          run({ id: "approval", paused_reason: "approval", roadmap_item_id: "c" }),
          run({ id: "question", paused_reason: "question", roadmap_item_id: "d" }),
        ],
        latestEvents: [event({ id: "ev", item_id: "e" })],
        projectHold: projectHold(),
      }),
    );
    // The two holds sit above the gates: Release is one click and needs no
    // evidence surface, and the board's hold stops the most.
    expect(cards.map((c) => c.reason)).toEqual([
      "workflow-question",
      "project-held",
      "item-held",
      "workflow-approval",
      "workflow-conflict",
      "workflow-budget",
      "item-blocked",
    ]);
  });

  it("breaks a bucket tie on recency, then on id", () => {
    const cards = buildNeedsYou(
      input({
        items: [item({ id: "a" }), item({ id: "b" }), item({ id: "c" })],
        runs: [
          run({ id: "old", roadmap_item_id: "a", updated_at: 10 }),
          run({ id: "new", roadmap_item_id: "b", updated_at: 20 }),
          // Same instant as `new`: the id decides, so the order never flickers.
          run({ id: "also", roadmap_item_id: "c", updated_at: 20 }),
        ],
      }),
    );
    expect(cards.map((c) => c.runId)).toEqual(["also", "new", "old"]);
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
