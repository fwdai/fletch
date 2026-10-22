import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import type { ActionTone } from "@desktop/components/RightPanel/primaryActions";
import { useStore } from "../../../store";
import { COMMIT_KEYS, MANUAL_KEY } from "./actions";
import type { ShipView } from "./derive";
import { useShipActions } from "./useShipActions";

/** The phone's button for each of the desktop footer's tones. */
const TONE_CLASS: Record<ActionTone, string> = {
  accent: "primary",
  ghost: "ghost",
  success: "success",
  merged: "primary",
  danger: "danger",
};

/** The action footer: the ladder's one primary button, and a quiet "More"
 *  under it when the overflow sheet has anything to offer. `commitOnly` is the
 *  Changes tab's view of the same footer — only a commit / push / open-PR
 *  primary and its manual alternative, since the PR's own remedies belong on
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
  const { run, label, disabled, isDelegate, busy } = useShipActions(agent, onDelegated);
  const { footer } = view;

  const primary =
    commitOnly && !(footer.primary && COMMIT_KEYS.has(footer.primary.key)) ? null : footer.primary;
  const items = footer.groups.flatMap((g) => g.items);
  const alts = !commitOnly ? items : primary ? items.filter((a) => a.key === MANUAL_KEY) : [];
  // A playbook in flight holds the button. A restricted footer waits only for
  // its own family: the Changes tab has no button to hold while the agent
  // fixes CI.
  const working = view.strip.kind === "working" && (!commitOnly || primary != null);
  if (!primary && !working && alts.length === 0) return null;

  return (
    <div className="ch-foot">
      {working ? (
        <button type="button" className="btn primary" disabled>
          <span className="working-dots">
            <i />
            <i />
            <i />
          </span>
          Agent working…
        </button>
      ) : primary ? (
        <>
          <button
            type="button"
            className={`btn ${TONE_CLASS[primary.tone ?? "accent"]}`}
            onClick={() => void run(primary.key)}
            disabled={disabled(primary)}
          >
            <Icon name={primary.icon} size={17} />
            {isDelegate(primary) && busy ? "Agent is busy…" : label(primary)}
          </button>
          {footer.mainReason && <div className="why">{footer.mainReason}</div>}
        </>
      ) : null}
      {commitOnly ? (
        alts.map((a) => (
          <button key={a.key} type="button" className="alt" onClick={() => void run(a.key)}>
            {a.label}
          </button>
        ))
      ) : alts.length > 0 ? (
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
