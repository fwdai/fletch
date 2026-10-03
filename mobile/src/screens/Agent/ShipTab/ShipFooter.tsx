import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon, type IconName } from "@desktop/components/Icon";
import { useStore } from "../../../store";
import { COMMIT_FAMILY, isCommitAction, type ShipAction, type ShipView } from "./derive";
import { useShipActions } from "./useShipActions";

export const ACTION_ICON: Record<ShipAction["kind"], IconName> = {
  delegate: "pr",
  merge: "merge",
  archive: "archive",
  github: "github",
  manual: "edit",
};

/** The action footer: the ladder's one primary button, and a quiet "More"
 *  under it when the overflow sheet has anything to offer. `commitOnly` is the
 *  Changes tab's view of the same footer — only the commit / push / open-PR
 *  playbook and its manual alternative, since the PR's own remedies belong on
 *  the Ship tab. Renders nothing when there is nothing to press. */
export function ShipFooter({
  agent,
  view,
  onDelegated,
  commitOnly = false,
}: {
  agent: AgentRecord;
  view: ShipView;
  /** Called once a git action has been handed to the agent, so the screen can
   *  show the chat where the agent's turn plays out. */
  onDelegated?: () => void;
  commitOnly?: boolean;
}) {
  const openSheet = useStore((s) => s.openSheet);
  const { run, label, disabled, busy } = useShipActions(agent, onDelegated);
  const working = view.strip.kind === "working";
  // A restricted footer waits only for its own family: the Changes tab has no
  // button to hold while the agent fixes CI.
  const workingOnCommit =
    working && view.rung.do === "delegate" && COMMIT_FAMILY.has(view.rung.kind);

  let primary = view.primary;
  let more = view.more;
  if (commitOnly) {
    primary = isCommitAction(primary) ? primary : null;
    more = primary ? more.filter((a) => a.kind === "manual") : [];
  }
  const showWorking = commitOnly ? workingOnCommit : working;
  if (!primary && !showWorking && more.length === 0) return null;

  return (
    <div className="ch-foot">
      {showWorking ? (
        <button type="button" className="btn primary" disabled>
          <span className="working-dots">
            <i />
            <i />
            <i />
          </span>
          Agent working…
        </button>
      ) : primary ? (
        <button
          type="button"
          className="btn primary"
          onClick={() => void run(primary)}
          disabled={disabled(primary)}
        >
          <Icon name={ACTION_ICON[primary.kind]} size={17} />
          {primary.kind === "delegate"
            ? busy
              ? "Agent is busy…"
              : `${label(primary)} with agent`
            : label(primary)}
        </button>
      ) : null}
      {commitOnly ? (
        more.map((a) => (
          <button key={a.key} type="button" className="alt" onClick={() => void run(a)}>
            {a.label}
          </button>
        ))
      ) : more.length > 0 ? (
        <button
          type="button"
          className="alt"
          onClick={() => openSheet("shipMore", { agentId: agent.id })}
        >
          More
        </button>
      ) : null}
    </div>
  );
}
