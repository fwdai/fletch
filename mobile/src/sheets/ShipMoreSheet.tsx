import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import { Fragment } from "react";
import { Sheet } from "../components/ui";
import { useShipActions } from "../screens/Agent/ShipTab/useShipActions";
import { useShipView } from "../screens/Agent/ShipTab/useShipView";
import { agentOf, useStore } from "../store";

/** The Ship footer's overflow: the desktop footer's menu, in its groups, less
 *  what the phone can't run — and the manual PR sheet behind a commit playbook.
 *  A row the gate closed stays, dimmed, with why. Same dispatch as the footer. */
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
  const { groups } = useShipView(agent).footer;
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
      {groups.map((g, i) => (
        <Fragment key={g.title ?? i}>
          {g.title && <div className="sect">{g.title}</div>}
          <div className="card" style={{ marginTop: 4 }}>
            {g.items.map((a) => (
              <button
                key={a.key}
                type="button"
                className="row"
                disabled={disabled(a)}
                onClick={() => {
                  // A merge's first tap only arms it; the sheet stays for the second.
                  void run(a.key).then((done) => done && onClose());
                }}
              >
                <span className="pm lg" style={{ background: "var(--bg-2)", color: "var(--fg-2)" }}>
                  <Icon name={a.icon} size={15} />
                </span>
                <div className="main">
                  <div className="lbl">{label(a)}</div>
                  {a.disabled && a.reason && <div className="sub">{a.reason}</div>}
                </div>
              </button>
            ))}
          </div>
        </Fragment>
      ))}
      {groups.length === 0 && (
        <div className="card" style={{ marginTop: 4 }}>
          <div className="empty">Nothing else to do here.</div>
        </div>
      )}
    </Sheet>
  );
}
