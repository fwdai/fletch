import type { AgentRecord } from "@/api";
import { ProviderIcon } from "@/components/ProviderIcon";
import { providerChip } from "@/data/providers";
import { type DotStatus, STATUS_LABEL } from "./derive";
import { IsolationDetail } from "./IsolationDetail";
import { isolationOf } from "./isolation";
import { StatusDot } from "./StatusDot";
import { WorkspaceActions } from "./WorkspaceActions";

/** The workspace segment's hover/focus popover: who + what, then where it
 *  runs — the isolation in two lines at most, for a box its live phase and
 *  the one action it takes here — and the move to the other sandbox. */
export function WorkspacePopover({ agent, status }: { agent: AgentRecord; status: DotStatus }) {
  const stamped = isolationOf(agent.sandbox_engine);

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

      <IsolationDetail agent={agent} />

      {stamped.kind !== "container" && (
        <WorkspaceActions agent={agent} onBox={stamped.kind === "box"} />
      )}
    </div>
  );
}
