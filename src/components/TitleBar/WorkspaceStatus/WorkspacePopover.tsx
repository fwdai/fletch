import type { AgentRecord } from "@/api";
import { ProviderIcon } from "@/components/ProviderIcon";
import { providerChip } from "@/data/providers";
import { useAppStore } from "@/store";
import { BoxActions } from "./BoxActions";
import { type DotStatus, STATUS_LABEL } from "./derive";
import { IsolationIcon } from "./IsolationIcon";
import { boxPhaseLabel, isolationOf } from "./isolation";
import { StatusDot } from "./StatusDot";

/** The workspace segment's hover/focus popover: who + what, then where it
 *  runs — the isolation, and for a box its live phase and the one action it
 *  takes here. */
export function WorkspacePopover({ agent, status }: { agent: AgentRecord; status: DotStatus }) {
  const boxState = useAppStore((s) => s.boxStates[agent.id]);
  const isolation = isolationOf(agent.sandbox_engine);
  const isBox = isolation.kind === "box";
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

      {isBox && <BoxActions agent={agent} />}
    </div>
  );
}
