import { describe, expect, it } from "vitest";
import type { RoadmapItem, RoadmapProposal } from "@/api";
import { applyBoardEvent, createBoardSync } from "./boardSync";

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

// ── applyBoardEvent ───────────────────────────────────────────────────────────

describe("applyBoardEvent", () => {
  it("appends an unseen row and replaces a known one by id", () => {
    const a = item({ id: "a" });
    const b = item({ id: "b" });
    const appended = applyBoardEvent([a], { kind: "upsert", row: b });
    expect(appended.map((r) => r.id)).toEqual(["a", "b"]);

    const renamed = item({ id: "a", title: "Renamed" });
    const replaced = applyBoardEvent(appended, { kind: "upsert", row: renamed });
    expect(replaced.map((r) => r.id)).toEqual(["a", "b"]);
    expect(replaced[0].title).toBe("Renamed");
  });

  it("drops the row a delete names and leaves the rest alone", () => {
    const rows = [item({ id: "a" }), item({ id: "b" })];
    expect(applyBoardEvent(rows, { kind: "delete", id: "a" }).map((r) => r.id)).toEqual(["b"]);
    expect(applyBoardEvent(rows, { kind: "delete", id: "zzz" }).map((r) => r.id)).toEqual([
      "a",
      "b",
    ]);
  });
});

// ── createBoardSync ───────────────────────────────────────────────────────────

describe("createBoardSync", () => {
  it("buffers events arriving before the snapshot instead of dropping them", () => {
    const s = store();
    const sync = createBoardSync(s.commit);

    // The PM proposes a row while the fetch is still in flight.
    sync.push({ kind: "upsert", row: item({ id: "ghost", status: "proposed" }) });
    expect(s.ids).toEqual([]); // nothing committed yet

    sync.settle([item({ id: "a" })]);
    expect(s.ids).toEqual(["a", "ghost"]);
  });

  it("replays the buffer in arrival order, upserts and deletes interleaved", () => {
    const s = store();
    const sync = createBoardSync(s.commit);

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
    const sync = createBoardSync(s.commit);

    sync.push({ kind: "delete", id: "gone" });
    // The backend read its rows before the delete landed.
    sync.settle([item({ id: "gone" }), item({ id: "kept" })]);
    expect(s.ids).toEqual(["kept"]);
  });

  it("applies events directly once settled", () => {
    const s = store();
    const sync = createBoardSync(s.commit);
    sync.settle([item({ id: "a" })]);

    sync.push({ kind: "upsert", row: item({ id: "b" }) });
    expect(s.ids).toEqual(["a", "b"]);
    sync.push({ kind: "delete", id: "a" });
    expect(s.ids).toEqual(["b"]);
  });

  it("keeps buffered events when the fetch failed and there is no snapshot", () => {
    const s = store([item({ id: "old" })]);
    const sync = createBoardSync(s.commit);

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

describe("createBoardSync over proposals", () => {
  it("buffers a proposal parked mid-load and replays it over the snapshot", () => {
    let rows: RoadmapProposal[] = [];
    const sync = createBoardSync<RoadmapProposal>((update) => {
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
    const sync = createBoardSync<RoadmapProposal>((update) => {
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
