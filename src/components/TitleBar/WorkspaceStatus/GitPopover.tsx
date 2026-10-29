import type { PrChecks } from "@/api";
import { Icon } from "@/components/Icon";
import { LaneRow } from "@/components/RightPanel/GitPanel/Lanes";
import type { Lane } from "@/components/RightPanel/GitPanel/laneModel";

interface Props {
  /** The checkout's lanes, exactly as the Git panel's header draws them. */
  lanes: Lane[];
  onOpenPanel: () => void;
}

/** The git segment's hover/focus popover: the checkout's lanes — the same
 *  rows as the Git panel's header, each open PR with its check breakdown —
 *  and one way in: the panel, where every action lives. The PR chip on a lane
 *  is the link to GitHub, as it is in the panel. */
export function GitPopover({ lanes, onOpenPanel }: Props) {
  return (
    <div className="ws-pop" role="dialog" aria-label="Git status">
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
