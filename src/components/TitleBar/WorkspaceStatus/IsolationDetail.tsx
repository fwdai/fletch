import type { AgentRecord } from "@/api";
import { useAppStore } from "@/store";
import { IsolationIcon } from "./IsolationIcon";
import { boxPhaseLabel, shownIsolation } from "./isolation";

/** Where a workspace runs, spelled out: the isolation in two lines at most
 *  and, for a box, its live phase and why it failed. While the workspace
 *  moves, everything here is about where it's going. */
export function IsolationDetail({ agent }: { agent: AgentRecord }) {
  const boxState = useAppStore((s) => s.boxStates[agent.id]);
  const movingTo = useAppStore((s) => s.movingWorkspace[agent.id] ?? null);
  const isolation = shownIsolation(agent.sandbox_engine, movingTo);
  const isBox = !movingTo && isolation.kind === "box";
  const failure = isBox && boxState?.phase === "failed" ? boxState.reason : null;

  return (
    <div className="ws-pop-iso">
      <IsolationIcon
        isolation={isolation}
        boxPhase={boxState?.phase}
        moving={movingTo !== null}
        size={14}
      />
      <div className="ws-pop-iso-text">
        <div className="ws-pop-iso-title">
          {isolation.title}
          {movingTo && <span className="ws-pop-iso-phase starting">Moving</span>}
          {isBox && boxState && (
            <span className={`ws-pop-iso-phase ${boxState.phase}`}>{boxPhaseLabel(boxState)}</span>
          )}
        </div>
        {failure && <div className="ws-pop-iso-fail">{failure}</div>}
        <div className="ws-pop-iso-hint">{isolation.hint}</div>
      </div>
    </div>
  );
}
