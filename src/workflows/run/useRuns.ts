// run/useRuns.ts — the reactive list of workflow runs for the sidebar and the
// roadmap board. Loads the runs (newest-updated first) and
// keeps the list live: `wf:run` fires the full row on every run-row change, so a
// launch or a status flip upserts in place. This hook is a pure view; the backend
// owns every run's lifecycle.
//
// The load subscribes *before* it fetches and replays what arrived in between
// (see rowSync.ts). Fetch-first lost terminal flips: the snapshot's wholesale
// replace clobbers a `wf:run` that landed while the request was in flight, and a
// run that has finished emits nothing further, so the stale `running` stayed on
// screen until something else moved the row. Every surface that reads a run's
// state (the sidebar row, the roadmap board's cards and rail) reads this list, so
// one lost event was wrong in all of them at once.

import { useEffect, useRef, useState } from "react";
import { useEngine } from "@/api/engine";
import { createRowSync, type RowSync } from "@/rowSync";
import type { WfRun } from "../../api";

/** Newest-updated first — the order `wf_list_runs` returns and every consumer
 *  draws. Re-applied on each change because the sequencer appends an unseen row
 *  (it cannot know the sort), and an upsert moves a row's `updated_at`. */
function newestFirst(rows: WfRun[]): WfRun[] {
  return [...rows].sort((a, b) => b.updated_at - a.updated_at);
}

/** The hook's ordering guarantee without React: a run-row sequencer that keeps
 *  the list sorted and hands each committed list to `commit`. Exported for the
 *  test — a lost terminal flip has no other observable surface. */
export function createRunSync(commit: (rows: WfRun[]) => void, held: WfRun[] = []): RowSync<WfRun> {
  // `held` is what a caller reading the list again already shows: a read that
  // fails then settles over it rather than over nothing.
  let rows: WfRun[] = held;
  return createRowSync<WfRun>((update) => {
    rows = newestFirst(update(rows));
    commit(rows);
  });
}

export function useRuns(reload = 0): WfRun[] {
  const { api, events } = useEngine();
  const [runs, setRuns] = useState<WfRun[]>([]);
  const shown = useRef(runs);

  // biome-ignore lint/correctness/useExhaustiveDependencies: `reload` is read by nothing in here; a new value is the caller asking for the list again.
  useEffect(() => {
    let alive = true;
    // Unmounting mid-load must not write state, so the commit is gated the same
    // way the fetch is.
    const sync = createRunSync((rows) => {
      if (!alive) return;
      shown.current = rows;
      setRuns(rows);
    }, shown.current);

    const off = events.onWfRun((row) => sync.push({ kind: "upsert", row }));
    const offDeleted = events.onWfRunDeleted((runId) => sync.push({ kind: "delete", id: runId }));

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
  }, [api, events, reload]);

  return runs;
}
