import type { AgentRecord, PrChecks } from "@/api";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { LaneRow } from "@/components/RightPanel/GitPanel/Lanes";
import type { Lane } from "@/components/RightPanel/GitPanel/laneModel";
import { providerChip } from "@/data/providers";
import { type DotStatus, STATUS_LABEL } from "./derive";
import { StatusDot } from "./StatusDot";

interface Props {
  agent: AgentRecord;
  status: DotStatus;
  /** The checkout's lanes, exactly as the Git panel's header draws them. */
  lanes: Lane[];
  onOpenPanel: () => void;
}

/** The hover/focus details popover: who + what, then the checkout's lanes —
 *  the same rows as the Git panel's header, each open PR with its check
 *  breakdown — and one way in: the panel, where every action lives. The PR
 *  chip on a lane is the link to GitHub, as it is in the panel. */
export function Popover({ agent, status, lanes, onOpenPanel }: Props) {
  return (
    <div className="ws-pop" role="dialog">
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

      <div className="ws-pop-lanes">
        {lanes.map((lane) => (
          <div key={lane.id} className="ws-pop-lane">
            <LaneRow lane={lane} trailing={lane.current ? <AheadBehind lane={lane} /> : null} />
            {lane.pr?.state.state === "open" && lane.pr.checks && lane.pr.checks.total > 0 && (
              <CheckDetail checks={lane.pr.checks} />
            )}
          </div>
        ))}
      </div>

      <div className="ws-pop-div" />
      <div className="ws-pop-actions">
        <button type="button" className="ws-act primary" onClick={onOpenPanel}>
          <Icon name="branch" size={12} /> Open Git panel
        </button>
      </div>
    </div>
  );
}

/** The tree's distance from its base and its upstream, on the lane it is on. */
function AheadBehind({ lane }: { lane: Lane }) {
  const git = lane.git;
  if (!git) return null;
  return (
    <span className="ws-pop-ab">
      <span className="up">
        <Icon name="arrowUp" size={10} />
        {git.ahead}
      </span>
      <span className="dn">
        <Icon name="arrowDown" size={10} />
        {git.behind}
      </span>
    </span>
  );
}

function CheckDetail({ checks }: { checks: PrChecks }) {
  return (
    <div className="ws-checkblock">
      <div className="ws-checkbar" role="img" aria-label="check status">
        {checks.passed > 0 && <i className="p" style={{ flex: checks.passed }} />}
        {checks.failed > 0 && <i className="f" style={{ flex: checks.failed }} />}
        {checks.pending > 0 && <i className="w" style={{ flex: checks.pending }} />}
      </div>
      <div className="ws-checkcounts">
        <span className="ok">
          <Icon name="check" size={10} />
          {checks.passed} passed
        </span>
        {checks.failed > 0 && (
          <span className="bad">
            <Icon name="close" size={10} />
            {checks.failed} failed
          </span>
        )}
        {checks.pending > 0 && (
          <span className="pend">
            <span className="ws-spin sm" />
            {checks.pending} running
          </span>
        )}
      </div>
    </div>
  );
}
