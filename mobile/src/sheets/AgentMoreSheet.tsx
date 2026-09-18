import { Icon } from "@desktop/components/Icon";
import { useEffect, useState } from "react";
import { Sheet } from "../components/ui";
import { isBusy } from "../lib/agents";
import { ignore } from "../lib/ignore";
import { agentOf, useStore } from "../store";

/** Stop, and then archive or delete. "Open on host", "Run script" and copying
 *  the branch are desktop-side actions with no remote op in the v1 allowlist.
 *
 *  A planning chat is deleted rather than archived: an archived purpose chat is
 *  in neither the snapshot nor `list_project_chats`, so it would be gone with no
 *  way to reach it again. A host without `discard_agent` therefore gets neither
 *  row for one. */
export function AgentMoreSheet({
  open,
  onClose,
  agentId,
}: {
  open: boolean;
  onClose: () => void;
  agentId?: string;
}) {
  const agent = useStore((s) => (agentId ? agentOf(s, agentId) : undefined));
  const stop = useStore((s) => s.stop);
  const archive = useStore((s) => s.archive);
  const deleteChat = useStore((s) => s.deleteChat);
  const canDiscard = useStore((s) => s.hostSupports("discard_agent"));
  // A delete cannot be undone, so the row arms on the first tap and only
  // deletes on the second. It holds the agent it was armed for — the sheet is
  // never unmounted, so an arming must not carry to the next agent — and
  // lapses whenever the sheet is closed.
  const [armed, setArmed] = useState<string | null>(null);
  useEffect(() => {
    if (!open) setArmed(null);
  }, [open]);
  if (!agent) return null;
  const purpose = !!agent.purpose;
  const isArmed = armed === agent.id;
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
              void stop(agent.id).catch(ignore);
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
        {purpose ? (
          canDiscard && (
            <button
              type="button"
              className="row"
              onClick={() => {
                if (!isArmed) return setArmed(agent.id);
                void deleteChat(agent.id).catch(ignore);
              }}
            >
              <span
                className="pm lg"
                style={{
                  background: "color-mix(in oklch,var(--danger),transparent 86%)",
                  color: "var(--danger)",
                }}
              >
                <Icon name="trash" size={15} />
              </span>
              <div className="main">
                <div className="lbl" style={{ color: "var(--danger)" }}>
                  {isArmed ? "Confirm delete" : "Delete chat"}
                </div>
                <div className="sub">
                  Removes the conversation and its checkout. Nothing on the board changes.
                </div>
              </div>
            </button>
          )
        ) : (
          <button
            type="button"
            className="row"
            onClick={() => void archive(agent.id).catch(ignore)}
          >
            <span className="pm lg" style={{ background: "var(--bg-2)", color: "var(--fg-2)" }}>
              <Icon name="archive" size={15} />
            </span>
            <div className="main">
              <div className="lbl">Archive agent</div>
              <div className="sub">Removes the worktree, keeps the branch</div>
            </div>
          </button>
        )}
      </div>
    </Sheet>
  );
}
