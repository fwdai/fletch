// run/RunRow.tsx — a workflow run as a sidebar row. Rendered with the exact
// same markup/classes as an agent row (AgentRow): a leading workflow glyph marks
// it as a run, the chip shows the current step's agent, and it can be stopped the
// same way as an agent.

import type { KeyboardEvent, MouseEvent } from "react";
import { Icon } from "../../components/Icon";
import { useAppStore } from "../../store";
import { formatAge } from "../../util/format";
import { useMinuteClock } from "../../util/hooks";
import { AgentAvatar } from "../builder/AgentAvatar";
import { resolveAgent } from "../shared";
import { cancelRun } from "./engine";
import type { WorkflowRun } from "./types";

export function RunRow({
  run,
  selected,
  onSelect,
}: {
  run: WorkflowRun;
  selected: boolean;
  onSelect: () => void;
}) {
  const customAgents = useAppStore((s) => s.customAgents);
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const now = useMinuteClock();

  const working = run.status === "running";
  const stoppable = run.status === "running" || run.status === "pending";
  // Same left-spine vocabulary as an agent row: live → accent, failed → danger,
  // everything else (pending/paused/done/canceled) → the faint idle grey.
  const railClass = working ? "run" : run.status === "failed" ? "err" : "idle";
  const age = formatAge(new Date(run.created_at).toISOString(), now);

  // The agent backing the current step (falls back to the first) — shown in the
  // same chip slot an agent row uses; the combine prefix is what marks it a run.
  const stepDef =
    run.steps_snapshot.find((s) => s.id === run.current_step_id) ?? run.steps_snapshot[0];
  const a = stepDef ? resolveAgent(stepDef.agent, customAgents, modelsByAgent) : null;

  const onStop = (e: MouseEvent) => {
    e.stopPropagation();
    void cancelRun(run.id);
  };

  return (
    <div
      className={`agent ${selected ? "active" : ""}`}
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
            {run.status === "failed" && <span className="ag-badge err">failed</span>}
          </span>
          <span className="ag-actions">
            {stoppable && (
              <button className="ag-act tip" data-tip="Stop" onClick={onStop} aria-label="Stop">
                <Icon name="stop" size={11} />
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
