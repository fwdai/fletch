import { useState } from "react";
import type { AgentRecord, WorkspaceKind } from "@/api";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { LINUX_VM, MACOS_SANDBOX } from "@/util/sandboxNames";

/** The escape hatch between sandboxes: move this workspace (its code, branch
 *  and conversation) to the other one. Offered here and nowhere else, on
 *  purpose: nothing in the app suggests it. Confirmed in place, since the
 *  agent restarts and ignored files don't travel. */
export function MoveSandbox({ agent, onBox }: { agent: AgentRecord; onBox: boolean }) {
  const [confirming, setConfirming] = useState(false);
  const moving = useAppStore((s) => s.movingWorkspace[agent.id] === true);
  const moveWorkspace = useAppStore((s) => s.moveWorkspace);
  const vmPossible = useAppStore((s) => s.boxProbe?.runtime_ok === true);
  const gate = useGate("moveSandbox");
  if (gate || (!onBox && !vmPossible)) return null;

  const target = onBox ? MACOS_SANDBOX : LINUX_VM;
  const kind: WorkspaceKind = onBox ? "host" : "box";
  const busy = agent.status === "running" || agent.status === "spawning";

  if (moving) {
    return (
      <div className="ws-pop-actions">
        <button type="button" className="ws-act" disabled>
          <Icon name={target.icon} size={12} /> Moving to {target.label}…
        </button>
      </div>
    );
  }

  if (confirming) {
    return (
      <div className="ws-pop-move">
        <div className="ws-pop-iso-hint">
          The agent restarts there. Ignored files (node_modules, .env) don't move.
        </div>
        <div className="ws-pop-actions">
          <button type="button" className="ws-act" onClick={() => setConfirming(false)}>
            Cancel
          </button>
          <button
            type="button"
            className="ws-act primary"
            onClick={() => {
              setConfirming(false);
              void moveWorkspace(agent.id, kind);
            }}
          >
            Move
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="ws-pop-actions">
      <button
        type="button"
        className="ws-act"
        disabled={busy}
        title={busy ? "Wait for the turn to finish." : undefined}
        onClick={() => setConfirming(true)}
      >
        <Icon name={target.icon} size={12} /> Move to {target.label}
      </button>
    </div>
  );
}
