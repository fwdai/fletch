import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import { Sheet } from "../components/ui";
import { ACTION_ICON } from "../screens/Agent/ShipTab/ShipFooter";
import { useShipActions } from "../screens/Agent/ShipTab/useShipActions";
import { useShipView } from "../screens/Agent/ShipTab/useShipView";
import { agentOf, useStore } from "../store";

/** The Ship footer's overflow: everything the ladder keeps reachable beside
 *  its one primary — Merge while the gate is open, the manual PR sheet behind
 *  a commit playbook, the PR on GitHub. Same dispatch as the footer. */
export function ShipMoreSheet({
  open,
  onClose,
  agentId,
}: {
  open: boolean;
  onClose: () => void;
  agentId?: string;
}) {
  const agent = useStore((s) => (agentId ? agentOf(s, agentId) : undefined));
  if (!agent) return null;
  return <Rows agent={agent} open={open} onClose={onClose} />;
}

/** Split out so the hooks have an agent to read for. */
function Rows({
  agent,
  open,
  onClose,
}: {
  agent: AgentRecord;
  open: boolean;
  onClose: () => void;
}) {
  const view = useShipView(agent);
  const { run, label, disabled } = useShipActions(agent, onClose);
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title="Ship"
      right={
        <button type="button" className="tbtn" onClick={onClose}>
          Done
        </button>
      }
    >
      <div className="card" style={{ marginTop: 4 }}>
        {view.more.map((a) => (
          <button
            key={a.key}
            type="button"
            className="row"
            disabled={disabled(a)}
            onClick={() => {
              // A merge's first tap only arms it; the sheet stays for the second.
              void run(a).then((done) => done && onClose());
            }}
          >
            <span className="pm lg" style={{ background: "var(--bg-2)", color: "var(--fg-2)" }}>
              <Icon name={ACTION_ICON[a.kind]} size={15} />
            </span>
            <div className="main">
              <div className="lbl">{label(a)}</div>
            </div>
          </button>
        ))}
        {view.more.length === 0 && <div className="empty">Nothing else to do here.</div>}
      </div>
    </Sheet>
  );
}
