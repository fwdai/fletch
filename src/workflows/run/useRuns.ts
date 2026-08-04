// run/useRuns.ts — the reactive list of workflow runs for the sidebar, the
// review queue, and the roadmap board. Loads the runs (newest-updated first) and
// keeps the list live: `wf:run` fires the full row on every run-row change, so a
// launch, a status flip, or a pause upserts in place. Run resumption after an app
// restart is owned by the backend (`resume_active_runs` on startup), so this hook
// is a pure view.
//
// The load subscribes *before* it fetches and replays what arrived in between
// (see rowSync.ts). Fetch-first lost pauses: the snapshot's wholesale replace
// clobbers a `wf:run` that landed while the request was in flight, and a paused
// run emits nothing further — it is waiting on a human — so the pause stayed
// invisible until something else moved the row. Every surface that decides from a
// pause (the sidebar badge, Mission Control, the roadmap's "Needs you" strip)
// reads this list, so one lost event was silent in all of them at once.

import { useEffect, useState } from "react";
import { createRowSync, type RowSync } from "@/rowSync";
import { api, onWfRun, onWfRunDeleted, type WfRun } from "../../api";

/** Newest-updated first — the order `wf_list_runs` returns and every consumer
 *  draws. Re-applied on each change because the sequencer appends an unseen row
 *  (it cannot know the sort), and an upsert moves a row's `updated_at`. */
function newestFirst(rows: WfRun[]): WfRun[] {
  return [...rows].sort((a, b) => b.updated_at - a.updated_at);
}

/** The hook's ordering guarantee without React: a run-row sequencer that keeps
 *  the list sorted and hands each committed list to `commit`. Exported for the
 *  test — a lost pause has no other observable surface. */
export function createRunSync(commit: (rows: WfRun[]) => void): RowSync<WfRun> {
  let rows: WfRun[] = [];
  return createRowSync<WfRun>((update) => {
    rows = newestFirst(update(rows));
    commit(rows);
  });
}

export function useRuns(): WfRun[] {
  const [runs, setRuns] = useState<WfRun[]>([]);

  useEffect(() => {
    let alive = true;
    // Unmounting mid-load must not write state, so the commit is gated the same
    // way the fetch is.
    const sync = createRunSync((rows) => {
      if (alive) setRuns(rows);
    });

    const off = onWfRun((row) => sync.push({ kind: "upsert", row }));
    const offDeleted = onWfRunDeleted((runId) => sync.push({ kind: "delete", id: runId }));

    void (async () => {
      // Registration has to be awaited, not just started: an event emitted
      // before `listen` resolves never reaches us at all.
      await Promise.all([off, offDeleted]);
      if (!alive) return;
      try {
        const rows = await api.wfListRuns();
        if (!alive) return;
        sync.settle(rows);
      } catch {
        // No snapshot to replay over — settle anyway so later events still apply
        // instead of piling up in the buffer forever.
        if (alive) sync.settle();
      }
    })();

    return () => {
      alive = false;
      void off.then((f) => f());
      void offDeleted.then((f) => f());
    };
  }, []);

  return runs;
}
