// run/useRuns.ts — the reactive list of workflow runs for the sidebar. Loads the
// runs (newest-updated first) and keeps the list live: `wf:run` fires the full
// row on every run-row change, so a launch, a status flip, or a pause upserts in
// place. Run resumption after an app restart is owned by the backend
// (`resume_active_runs` on startup), so this hook is a pure view.

import { useEffect, useState } from "react";
import { api, onWfRun, onWfRunDeleted, type WfRun } from "../../api";

export function useRuns(): WfRun[] {
  const [runs, setRuns] = useState<WfRun[]>([]);

  useEffect(() => {
    let alive = true;
    api
      .wfListRuns()
      .then((rows) => {
        if (alive) setRuns(rows);
      })
      .catch(() => {});

    const off = onWfRun((row) => {
      setRuns((prev) => {
        const next = prev.some((r) => r.id === row.id)
          ? prev.map((r) => (r.id === row.id ? row : r))
          : [row, ...prev];
        next.sort((a, b) => b.updated_at - a.updated_at);
        return next;
      });
    });
    const offDeleted = onWfRunDeleted((runId) => {
      setRuns((prev) => prev.filter((r) => r.id !== runId));
    });

    return () => {
      alive = false;
      void off.then((f) => f());
      void offDeleted.then((f) => f());
    };
  }, []);

  return runs;
}
