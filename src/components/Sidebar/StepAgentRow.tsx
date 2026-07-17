import type { KeyboardEvent } from "react";
import { AgentIdentityChip } from "@/components/AgentIdentityChip";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import type { StepChild } from "./stepChildren";

/** A run's step agent as a sidebar child of its RunRow — visually subordinate
 *  (indented, quieter) but speaking the same status vocabulary as `AgentRow`.
 *  Clicking focuses that step's chat in the run monitor. Step agents are
 *  capability-restricted and owned by the run, so this row carries no
 *  stop/archive/promote actions — its lifecycle is the run's. */
export function StepAgentRow({ runId, child }: { runId: string; child: StepChild }) {
  const { agent, rail, working } = child;
  const selectRunStep = useAppStore((s) => s.selectRunStep);

  const onSelect = () => selectRunStep(runId, agent.id);

  return (
    <div
      className="agent run-step"
      role="button"
      tabIndex={0}
      onClick={onSelect}
      onKeyDown={(e: KeyboardEvent) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
    >
      <span className={`ag-rail ${rail}`} />
      <div className="agent-row flex-center">
        <span className={`ag-name ${working ? "shimmer" : ""}`}>{agent.name}</span>
        <AgentIdentityChip agent={agent} />
        <span className="ag-slot iflex-center">
          <span className="ag-meta">
            {working && <span className="ag-loader" aria-label="Working" />}
            {agent.status === "error" && (
              <span className="ag-waiting tip" data-tip="Step failed" aria-label="Step failed">
                <Icon name="close" size={12} />
              </span>
            )}
          </span>
        </span>
      </div>
    </div>
  );
}
