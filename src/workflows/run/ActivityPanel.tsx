// run/ActivityPanel.tsx — the workflow run's side panel, mounted by App in the
// same right rail the agent panels (Code / Git / Run / Terminal) use, so a run
// gets the layout primitives an agent already has: the ⌘] toggle, the splitter,
// the persisted width. One tab for now — Activity, the run's event log.

import { Icon } from "../../components/Icon";
import { Timeline } from "./Timeline";
import { useRunEvents } from "./useRunEvents";
import { useRuns } from "./useRuns";

export function RunActivityPanel({ runId }: { runId: string }) {
  const { events } = useRunEvents(runId);

  const runs = useRuns();
  const run = runs.find((r) => r.id === runId);
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
        <Timeline events={events} />
      </div>
    </>
  );
}
