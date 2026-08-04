// The run list's load ordering. The bug these cover: `useRuns` fetched its
// snapshot and subscribed concurrently, so a `wf:run` that arrived while the
// fetch was in flight was clobbered by the wholesale replace. For a *paused* run
// there is no follow-up event to repair that — it is waiting on a human — so the
// pause stayed invisible until something else moved the row, in the sidebar, in
// Mission Control, and on the roadmap board at once.

import { describe, expect, it } from "vitest";
import type { WfRun } from "@/api";
import { createRunSync } from "./useRuns";

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
    status: "running",
    paused_reason: null,
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

/** Drive the sequencer and keep the last committed list. */
function sink() {
  let rows: WfRun[] = [];
  return {
    sync: createRunSync((next) => {
      rows = next;
    }),
    get rows() {
      return rows;
    },
    get ids() {
      return rows.map((r) => r.id);
    },
  };
}

describe("createRunSync", () => {
  it("keeps a pause that arrived while the snapshot was in flight", () => {
    const s = sink();

    // The run pauses on a question the moment after the backend read its rows.
    s.sync.push({
      kind: "upsert",
      row: run({ id: "a", status: "paused", paused_reason: "question", updated_at: 5 }),
    });
    expect(s.rows).toEqual([]); // nothing committed until the snapshot lands

    // The stale snapshot still says `running` — the replay must win, because no
    // further `wf:run` is coming for a run that is waiting on a human.
    s.sync.settle([run({ id: "a", status: "running", updated_at: 1 })]);

    expect(s.rows).toHaveLength(1);
    expect(s.rows[0].status).toBe("paused");
    expect(s.rows[0].paused_reason).toBe("question");
  });

  it("does not let the snapshot resurrect a run deleted during the load", () => {
    const s = sink();
    s.sync.push({ kind: "delete", id: "gone" });
    s.sync.settle([run({ id: "gone" }), run({ id: "kept" })]);
    expect(s.ids).toEqual(["kept"]);
  });

  it("orders newest-updated first, snapshot and live rows alike", () => {
    const s = sink();
    s.sync.settle([run({ id: "old", updated_at: 10 }), run({ id: "older", updated_at: 5 })]);
    expect(s.ids).toEqual(["old", "older"]);

    // A brand-new run: appended by the sequencer, sorted to the top here.
    s.sync.push({ kind: "upsert", row: run({ id: "fresh", updated_at: 20 }) });
    expect(s.ids).toEqual(["fresh", "old", "older"]);

    // An existing run touched: replaced in place, then re-sorted.
    s.sync.push({ kind: "upsert", row: run({ id: "older", updated_at: 30 }) });
    expect(s.ids).toEqual(["older", "fresh", "old"]);
  });

  it("still applies live events when the fetch failed", () => {
    const s = sink();
    s.sync.push({ kind: "upsert", row: run({ id: "a" }) });
    s.sync.settle(); // error path: no snapshot to replay over

    expect(s.ids).toEqual(["a"]);
    s.sync.push({ kind: "upsert", row: run({ id: "b", updated_at: 2 }) });
    expect(s.ids).toEqual(["b", "a"]);
    s.sync.push({ kind: "delete", id: "b" });
    expect(s.ids).toEqual(["a"]);
  });
});
