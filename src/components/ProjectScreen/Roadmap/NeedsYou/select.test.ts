import { describe, expect, it } from "vitest";
import type { RoadmapItem, RoadmapItemEvent, WfPausedReason, WfRun } from "@/api";
import { buildNeedsYou, latestByItem, mergeLatest, upsertLatest } from "./select";

// ── fixtures ──────────────────────────────────────────────────────────────────

function item(over: Partial<RoadmapItem> & { id: string }): RoadmapItem {
  return {
    project_id: "p1",
    code: `MCA-${over.id}`,
    parent_id: null,
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
    created_at: 0,
    updated_at: 0,
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
        items,
        runs: [
          run({ id: "budget", paused_reason: "budget_exceeded", roadmap_item_id: "a" }),
          run({ id: "conflict", paused_reason: "conflict", roadmap_item_id: "b" }),
          run({ id: "approval", paused_reason: "approval", roadmap_item_id: "c" }),
          run({ id: "question", paused_reason: "question", roadmap_item_id: "d" }),
        ],
        latestEvents: [event({ id: "ev", item_id: "e" })],
      }),
    );
    expect(cards.map((c) => c.reason)).toEqual([
      "workflow-question",
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
