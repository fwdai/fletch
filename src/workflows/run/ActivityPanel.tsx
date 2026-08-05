// run/ActivityPanel.tsx — the workflow run's side panel, mounted by App in the
// same right rail the agent panels (Code / Git / Run / Terminal) use, so a run
// gets the layout primitives an agent already has: the ⌘/ toggle, the splitter,
// the persisted width. One tab for now — Activity, the run's event log — with
// the sub-runs of a composed run (§10.3) listed above it as navigation.

import { useMemo } from "react";
import { Icon } from "../../components/Icon";
import { useAppStore } from "../../store";
import { runChip } from "./status";
import { Timeline } from "./Timeline";
import { useRunEvents } from "./useRunEvents";
import { useRuns } from "./useRuns";

export function RunActivityPanel({ runId }: { runId: string }) {
  const { events } = useRunEvents(runId);
  const selectRun = useAppStore((s) => s.selectRun);

  const runs = useRuns();
  const run = runs.find((r) => r.id === runId);
  const subRuns = useMemo(() => runs.filter((r) => r.parent_run_id === runId), [runs, runId]);
  const live = run?.status === "running";

  return (
    <>
      <div className="right-h flex-center">
        <div className="right-tabs">
          <button type="button" className="r-tab iflex-center text-sm active">
            <Icon name="activity" />
            Activity
            {live && <span className="r-tab-live-dot" />}
          </button>
        </div>
      </div>
      <div className="right-body">
        {subRuns.length > 0 && (
          <div className="wf-subruns">
            <div className="wf-side-head">Sub-runs</div>
            {subRuns.map((sr) => {
              const c = runChip(sr.status);
              return (
                <button
                  key={sr.id}
                  type="button"
                  className="wf-subrun-row"
                  onClick={() => selectRun(sr.id)}
                >
                  <span className="wf-srow-dot" style={{ background: c.tone }} />
                  <span className="wf-subrun-name">{sr.name}</span>
                  <span className="wf-subrun-status" style={{ color: c.tone }}>
                    {c.label}
                  </span>
                </button>
              );
            })}
          </div>
        )}
        <Timeline events={events} />
      </div>
    </>
  );
}
