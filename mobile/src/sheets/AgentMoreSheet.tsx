import { Icon } from "../components/Icon";
import { Sheet } from "../components/ui";
import { isBusy } from "../lib/agents";
import { agentOf, useStore } from "../store";

/** Stop and archive only. "Open on host", "Run script" and copying the branch
 *  are desktop-side actions with no remote op in the v1 allowlist. */
export function AgentMoreSheet({
  open,
  onClose,
  agentId,
}: {
  open: boolean;
  onClose: () => void;
  agentId?: string;
}) {
  const agent = useStore((s) => (agentId ? agentOf(s.workspace, agentId) : undefined));
  const stop = useStore((s) => s.stop);
  const archive = useStore((s) => s.archive);
  if (!agent) return null;
  return (
    <Sheet
      open={open}
      onClose={onClose}
      title={agent.name}
      right={
        <button type="button" className="tbtn" onClick={onClose}>
          Done
        </button>
      }
    >
      <div className="card" style={{ marginTop: 4 }}>
        {isBusy(agent) && (
          <button
            type="button"
            className="row"
            onClick={() => {
              onClose();
              void stop(agent.id);
            }}
          >
            <span
              className="pm lg"
              style={{
                background: "color-mix(in oklch,var(--danger),transparent 86%)",
                color: "var(--danger)",
              }}
            >
              <Icon name="stop" size={13} />
            </span>
            <div className="main">
              <div className="lbl" style={{ color: "var(--danger)" }}>
                Stop agent
              </div>
              <div className="sub">Interrupts the running turn</div>
            </div>
          </button>
        )}
        <button type="button" className="row" onClick={() => void archive(agent.id)}>
          <span className="pm lg" style={{ background: "var(--bg-2)", color: "var(--fg-2)" }}>
            <Icon name="archive" size={15} />
          </span>
          <div className="main">
            <div className="lbl">Archive agent</div>
            <div className="sub">Removes the worktree, keeps the branch</div>
          </div>
        </button>
      </div>
    </Sheet>
  );
}
