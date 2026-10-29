import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { providerChip } from "@/data/providers";
import { useAppStore } from "@/store";
import { type DotStatus, STATUS_LABEL } from "./derive";
import { IsolationIcon } from "./IsolationIcon";
import { boxPhaseLabel, isolationOf } from "./isolation";
import { StatusDot } from "./StatusDot";

/** The workspace segment's hover/focus popover: who + what, then where it
 *  runs — the isolation, and for a box its live phase and the one action it
 *  takes here. */
export function WorkspacePopover({ agent, status }: { agent: AgentRecord; status: DotStatus }) {
  const boxState = useAppStore((s) => s.boxStates[agent.id]);
  const stopBox = useAppStore((s) => s.stopBox);
  const isolation = isolationOf(agent.sandbox_engine);
  const isBox = isolation.kind === "box";
  // A box outlives the agent's process (its registry keeps it up until the
  // idle timer fires): while it is up the user can stop it here without
  // archiving the workspace.
  const boxUp = isBox && (boxState?.phase === "running" || boxState?.phase === "starting");
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
        <IsolationIcon isolation={isolation} boxPhase={boxState?.phase} size={14} />
        <div className="ws-pop-iso-text">
          <div className="ws-pop-iso-title">
            {isolation.title}
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

      {boxUp && (
        <div className="ws-pop-actions">
          <button type="button" className="ws-act" onClick={() => void stopBox(agent.id)}>
            <Icon name="stop" size={12} /> Stop box
          </button>
        </div>
      )}
    </div>
  );
}
