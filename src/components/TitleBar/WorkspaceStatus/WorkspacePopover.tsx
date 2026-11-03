import type { AgentRecord } from "@/api";
import { ProviderIcon } from "@/components/ProviderIcon";
import { providerChip } from "@/data/providers";
import { useAppStore } from "@/store";
import { type DotStatus, STATUS_LABEL } from "./derive";
import { IsolationIcon } from "./IsolationIcon";
import { boxPhaseLabel, isolationOf, shownIsolation } from "./isolation";
import { StatusDot } from "./StatusDot";
import { WorkspaceActions } from "./WorkspaceActions";

/** The workspace segment's hover/focus popover: who + what, then where it
 *  runs — the isolation in two lines at most, for a box its live phase and
 *  the one action it takes here — and the move to the other sandbox. */
export function WorkspacePopover({ agent, status }: { agent: AgentRecord; status: DotStatus }) {
  const boxState = useAppStore((s) => s.boxStates[agent.id]);
  const movingTo = useAppStore((s) => s.movingWorkspace[agent.id] ?? null);
  const stamped = isolationOf(agent.sandbox_engine);
  // While it moves, everything here is about where it's going.
  const isolation = shownIsolation(agent.sandbox_engine, movingTo);
  const isBox = !movingTo && isolation.kind === "box";
  const failure = isBox && boxState?.phase === "failed" ? boxState.reason : null;

  return (
    <div className="ws-pop" role="dialog" aria-label={`${agent.name} workspace`}>
      <div className="ws-pop-head">
        <StatusDot status={status} big />
        <div className="ws-pop-id">
          <div className="ws-pop-name">{agent.name}</div>
          <div className="ws-pop-sub">
            {STATUS_LABEL[status]}
            {agent.task && <> · {agent.task}</>}
          </div>
        </div>
        <ProviderIcon slug={agent.provider} {...providerChip(agent.provider)} size={22} />
      </div>

      <div className="ws-pop-div" />

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
              <span className={`ws-pop-iso-phase ${boxState.phase}`}>
                {boxPhaseLabel(boxState)}
              </span>
            )}
          </div>
          {failure && <div className="ws-pop-iso-fail">{failure}</div>}
          <div className="ws-pop-iso-hint">{isolation.hint}</div>
        </div>
      </div>

      {stamped.kind !== "container" && (
        <WorkspaceActions agent={agent} onBox={stamped.kind === "box"} />
      )}
    </div>
  );
}
