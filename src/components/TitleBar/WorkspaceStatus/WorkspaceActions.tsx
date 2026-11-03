import { useState } from "react";
import type { AgentRecord, WorkspaceKind } from "@/api";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { LINUX_VM, MACOS_SANDBOX } from "@/util/sandboxNames";
import { BoxActions } from "./BoxActions";

/** The popover's action row: a box's own action beside the escape hatch
 *  between sandboxes, which moves this workspace (its code, branch and
 *  conversation) to the other one. Offered here and nowhere else, on
 *  purpose: nothing in the app suggests it. Confirmed in place, the row
 *  giving way to the one line that says what it costs. */
export function WorkspaceActions({ agent, onBox }: { agent: AgentRecord; onBox: boolean }) {
  const [confirming, setConfirming] = useState(false);
  const movingTo = useAppStore((s) => s.movingWorkspace[agent.id] ?? null);
  const moving = movingTo !== null;
  const moveWorkspace = useAppStore((s) => s.moveWorkspace);
  const vmPossible = useAppStore((s) => s.boxProbe?.runtime_ok === true);
  const gate = useGate("moveSandbox");
  const offered = !gate && (onBox || vmPossible);

  // Where it's going: the move in flight's own target (the record is
  // restamped before it ends), else the other sandbox.
  const kind: WorkspaceKind = movingTo ?? (onBox ? "host" : "box");
  const target = kind === "box" ? LINUX_VM : MACOS_SANDBOX;
  const busy = agent.status === "running" || agent.status === "spawning";

  if (confirming && !moving) {
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
      {onBox && !moving && <BoxActions agent={agent} />}
      {offered && (
        <button
          type="button"
          className="ws-act"
          disabled={busy || moving}
          title={busy ? "Wait for the turn to finish." : undefined}
          onClick={() => setConfirming(true)}
        >
          <Icon name={target.icon} size={12} />
          {moving ? `Moving to ${target.label}…` : `Move to ${target.label}`}
        </button>
      )}
    </div>
  );
}
