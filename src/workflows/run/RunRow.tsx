// run/RunRow.tsx — a workflow run as a sidebar row. Rendered with the exact
// same markup/classes as an agent row (AgentRow): a leading workflow glyph marks
// it as a run, a chip shows the flow's lead agent, a paused-reason badge names
// why it's waiting, and it can be stopped the same way as an agent.

import { type KeyboardEvent, type MouseEvent, useState } from "react";
import { api, type WfRun } from "../../api";
import { Icon } from "../../components/Icon";
import { useAppStore } from "../../store";
import { formatAge } from "../../util/format";
import { useMinuteClock } from "../../util/hooks";
import { AgentAvatar } from "../builder/AgentAvatar";
import { resolveAgent } from "../shared";
import type { Spec } from "../spec";
import { flattenSteps } from "./RunView/flatten";
import { pausedLabel } from "./status";

export function RunRow({
  run,
  selected,
  onSelect,
  nested = false,
}: {
  run: WfRun;
  selected: boolean;
  onSelect: () => void;
  /** A composed sub-run (§10.3), rendered indented under its parent run. */
  nested?: boolean;
}) {
  const customAgents = useAppStore((s) => s.customAgents);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const setLastError = useAppStore((s) => s.setLastError);
  const now = useMinuteClock();

  const working = run.status === "running";
  const stoppable = run.status === "running" || run.status === "pending";
  // Delete is the inverse gate (§13: terminal runs only). It is destructive and
  // irreversible — the run's step-agent chats go with it — so it takes two
  // clicks: the first arms the button and the tooltip states what is lost.
  const deletable =
    run.status === "done" || run.status === "failed" || run.status === "canceled";
  const [confirmDelete, setConfirmDelete] = useState(false);
  // Same left-spine vocabulary as an agent row: live → accent, failed → danger,
  // everything else (pending/paused/done/canceled) → the faint idle grey.
  const railClass = working ? "run" : run.status === "failed" ? "err" : "idle";
  const age = formatAge(new Date(run.created_at).toISOString(), now);

  // The flow's lead (first) agent — a representative chip, the combine prefix is
  // what marks the row as a run. Resolved from the launch-snapshot spec.
  const spec = run.spec as Spec | null;
  const first = flattenSteps(spec)[0];
  const a = first
    ? resolveAgent(
        spec?.agents?.[first.agentAlias]?.custom_agent ??
          spec?.agents?.[first.agentAlias]?.base ??
          first.agentAlias,
        customAgents,
        modelsByAgent,
      )
    : null;

  const onStop = async (e: MouseEvent) => {
    e.stopPropagation();
    try {
      await api.wfCancel(run.id);
    } catch (err) {
      setLastError(`Failed to stop run: ${err}`);
    }
  };

  const onDelete = async (e: MouseEvent) => {
    e.stopPropagation();
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    try {
      await api.wfDeleteRun(run.id);
    } catch (err) {
      setLastError(`Failed to delete run: ${err}`);
      setConfirmDelete(false);
    }
  };

  return (
    <div
      className={`agent ${selected ? "active" : ""} ${nested ? "run-nested" : ""}`}
      role="button"
      tabIndex={0}
      aria-current={selected ? "page" : undefined}
      onClick={onSelect}
      onKeyDown={(e: KeyboardEvent) => {
        // Ignore keys bubbling from the nested stop button.
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
      onMouseLeave={() => setConfirmDelete(false)}
    >
      <span className={`ag-rail ${railClass}`} />
      <div className="agent-row">
        <span className="ag-wf-prefix tip" data-tip="Workflow" data-tip-down="">
          <Icon name="combine" size={12} />
        </span>
        <span className={`ag-name ${working ? "shimmer" : ""}`}>{run.name}</span>
        {a && (
          <span className="ag-prov-chip">
            <AgentAvatar
              custom={a.custom}
              slug={a.providerId}
              short={a.short}
              hue={a.hue}
              size={14}
            />
          </span>
        )}
        <span className="ag-slot">
          <span className="ag-meta">
            {working && <span className="ag-loader" aria-label="Working" />}
            {run.status === "paused" && run.paused_reason && (
              <span className="ag-badge warn">{pausedLabel(run.paused_reason)}</span>
            )}
            {run.status === "failed" && <span className="ag-badge err">failed</span>}
          </span>
          <span className="ag-actions">
            {stoppable && (
              <button
                className="ag-act tip"
                data-tip="Stop"
                onClick={(e) => void onStop(e)}
                aria-label="Stop"
              >
                <Icon name="stop" size={11} />
              </button>
            )}
            {deletable && (
              <button
                className={`ag-act tip ${confirmDelete ? "confirm-del" : ""}`}
                data-tip={
                  confirmDelete
                    ? "Deletes this run's chats too — click again to confirm"
                    : "Delete run"
                }
                onClick={(e) => void onDelete(e)}
                aria-label="Delete run"
              >
                <Icon name="trash" size={11} />
              </button>
            )}
          </span>
        </span>
      </div>
      <div className="agent-sub">
        <span className="a-task">{run.task || "workflow run"}</span>
        <span className="a-time">{age}</span>
      </div>
    </div>
  );
}
