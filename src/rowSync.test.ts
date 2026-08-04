import { describe, expect, it } from "vitest";
import type { RoadmapItem, RoadmapProposal } from "@/api";
import { applyRowEvent, createRowSync, createSingleSync } from "./rowSync";

// ── fixtures ──────────────────────────────────────────────────────────────────

function item(over: Partial<RoadmapItem> & { id: string }): RoadmapItem {
  return {
    project_id: "p1",
    code: `FLT-${over.id}`,
    parent_id: null,
    title: `Item ${over.id}`,
    why: "",
    horizon: "later",
    status: "open",
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

/** A `setRows`-shaped sink, so the sequencer can be driven without React. */
function store(initial: RoadmapItem[] = []) {
  let rows = initial;
  return {
    commit: (update: (prev: RoadmapItem[]) => RoadmapItem[]) => {
      rows = update(rows);
    },
    get ids() {
      return rows.map((r) => r.id);
    },
    get rows() {
      return rows;
    },
  };
}

// ── applyRowEvent ───────────────────────────────────────────────────────────

describe("applyRowEvent", () => {
  it("appends an unseen row and replaces a known one by id", () => {
    const a = item({ id: "a" });
    const b = item({ id: "b" });
    const appended = applyRowEvent([a], { kind: "upsert", row: b });
    expect(appended.map((r) => r.id)).toEqual(["a", "b"]);

    const renamed = item({ id: "a", title: "Renamed" });
    const replaced = applyRowEvent(appended, { kind: "upsert", row: renamed });
    expect(replaced.map((r) => r.id)).toEqual(["a", "b"]);
    expect(replaced[0].title).toBe("Renamed");
  });

  it("drops the row a delete names and leaves the rest alone", () => {
    const rows = [item({ id: "a" }), item({ id: "b" })];
    expect(applyRowEvent(rows, { kind: "delete", id: "a" }).map((r) => r.id)).toEqual(["b"]);
    expect(applyRowEvent(rows, { kind: "delete", id: "zzz" }).map((r) => r.id)).toEqual(["a", "b"]);
  });
});

// ── createRowSync ───────────────────────────────────────────────────────────

describe("createRowSync", () => {
  it("buffers events arriving before the snapshot instead of dropping them", () => {
    const s = store();
    const sync = createRowSync(s.commit);

    // The PM proposes a row while the fetch is still in flight.
    sync.push({ kind: "upsert", row: item({ id: "ghost", status: "proposed" }) });
    expect(s.ids).toEqual([]); // nothing committed yet

    sync.settle([item({ id: "a" })]);
    expect(s.ids).toEqual(["a", "ghost"]);
  });

  it("replays the buffer in arrival order, upserts and deletes interleaved", () => {
    const s = store();
    const sync = createRowSync(s.commit);

    sync.push({ kind: "upsert", row: item({ id: "x", title: "first" }) });
    sync.push({ kind: "upsert", row: item({ id: "y" }) });
    sync.push({ kind: "delete", id: "y" });
    sync.push({ kind: "upsert", row: item({ id: "x", title: "second" }) });

    sync.settle([]);
    expect(s.ids).toEqual(["x"]);
    expect(s.rows[0].title).toBe("second");
  });

  it("does not let a stale snapshot resurrect a row deleted during the load", () => {
    const s = store();
    const sync = createRowSync(s.commit);

    sync.push({ kind: "delete", id: "gone" });
    // The backend read its rows before the delete landed.
    sync.settle([item({ id: "gone" }), item({ id: "kept" })]);
    expect(s.ids).toEqual(["kept"]);
  });

  it("applies events directly once settled", () => {
    const s = store();
    const sync = createRowSync(s.commit);
    sync.settle([item({ id: "a" })]);

    sync.push({ kind: "upsert", row: item({ id: "b" }) });
    expect(s.ids).toEqual(["a", "b"]);
    sync.push({ kind: "delete", id: "a" });
    expect(s.ids).toEqual(["b"]);
  });

  it("keeps buffered events when the fetch failed and there is no snapshot", () => {
    const s = store([item({ id: "old" })]);
    const sync = createRowSync(s.commit);

    sync.push({ kind: "upsert", row: item({ id: "live" }) });
    sync.settle(); // error path: settle with nothing to replay over

    expect(s.ids).toEqual(["old", "live"]);
    sync.push({ kind: "delete", id: "old" });
    expect(s.ids).toEqual(["live"]);
  });
});

// ── the proposal stream ───────────────────────────────────────────────────────
// The PM's pending proposals ride a second instance of the same sequencer: the
// sequencer is generic, and a replaced proposal arrives as an upsert under the
// *same id* (the backend keeps it stable), so replace-by-id is the whole story.

function proposal(over: Partial<RoadmapProposal> & { id: string }): RoadmapProposal {
  return {
    item_id: `item-${over.id}`,
    project_id: "p1",
    kind: "update",
    patch: { title: "Retitled" },
    note: null,
    created_at: 0,
    ...over,
  };
}

describe("createRowSync over proposals", () => {
  it("buffers a proposal parked mid-load and replays it over the snapshot", () => {
    let rows: RoadmapProposal[] = [];
    const sync = createRowSync<RoadmapProposal>((update) => {
      rows = update(rows);
    });

    // The PM revises its ask while the fetch is in flight: same id, new
    // contents. The snapshot still carries the old ask; the replay must win.
    sync.push({ kind: "upsert", row: proposal({ id: "p", note: "revised" }) });
    sync.push({ kind: "delete", id: "ruled" });
    sync.settle([proposal({ id: "p", note: "stale" }), proposal({ id: "ruled" })]);

    expect(rows.map((r) => r.id)).toEqual(["p"]);
    expect(rows[0].note).toBe("revised");
  });

  it("swaps a replaced ask in place once settled — never two for one item", () => {
    let rows: RoadmapProposal[] = [];
    const sync = createRowSync<RoadmapProposal>((update) => {
      rows = update(rows);
    });
    sync.settle([proposal({ id: "p" })]);

    sync.push({ kind: "upsert", row: proposal({ id: "p", kind: "discard", patch: null }) });
    expect(rows).toHaveLength(1);
    expect(rows[0].kind).toBe("discard");

    sync.push({ kind: "delete", id: "p" });
    expect(rows).toEqual([]);
  });
});

describe("createSingleSync", () => {
  /** The board's hold: one row or none, keyed by project. */
  const hold = (reason: string) => ({ reason });

  it("replays a value that arrived mid-load over the snapshot", () => {
    let held: { reason: string } | null | undefined;
    const sync = createSingleSync<{ reason: string }>((v) => {
      held = v;
    });

    // The PM pulled the brake while the board was still loading.
    sync.push(hold("re-planning"));
    sync.settle(null);
    expect(held).toEqual(hold("re-planning"));

    // After settling, events apply straight through.
    sync.push(null);
    expect(held).toBeNull();
  });

  it("does not let a stale snapshot resurrect something released mid-load", () => {
    // The distinction between `undefined` ("nothing arrived") and `null` ("it was
    // lifted") is the whole point: the backend read the hold, the user released
    // it, and the snapshot landed afterwards.
    let held: { reason: string } | null | undefined = hold("stale");
    const sync = createSingleSync<{ reason: string }>((v) => {
      held = v;
    });
    sync.push(null);
    sync.settle(hold("stale"));
    expect(held).toBeNull();
  });

  it("leaves the held value alone when the fetch failed and nothing arrived", () => {
    let commits = 0;
    const sync = createSingleSync<{ reason: string }>(() => {
      commits += 1;
    });
    sync.settle();
    expect(commits, "no snapshot and no event is nothing to say").toBe(0);

    // …but a value that did arrive during the failed fetch still lands.
    const second = createSingleSync<{ reason: string }>(() => {
      commits += 1;
    });
    second.push(hold("held anyway"));
    second.settle();
    expect(commits).toBe(1);
  });
});
