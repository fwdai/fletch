import type { KeyboardEvent } from "react";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { formatDuration } from "@/util/format";
import { failedTip, type SubagentChild } from "./subagentChildren";

/** One of an agent's backgrounded sub-agents as a sidebar child of its
 *  AgentRow — the same indented, quieter row a workflow run's step agents get
 *  (StepAgentRow), speaking the same rail / shimmer / loader vocabulary.
 *  Clicking selects the agent and reveals the Task row that launched it in the
 *  chat. The task's lifecycle is Claude's, so the row carries no actions. */
export function SubagentRow({ agentId, child }: { agentId: string; child: SubagentChild }) {
  const focusToolCall = useAppStore((s) => s.focusToolCall);
  const onSelect = () => focusToolCall(agentId, child.task.toolUseId);

  return (
    <div
      className="agent run-step subagent no-actions"
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
      <span className={`ag-rail ${child.failed ? "err" : "run"}`} />
      <div className="agent-row flex-center">
        <span className={`ag-name ${child.running ? "shimmer" : ""}`} title={child.label}>
          {child.label}
        </span>
        {child.quietMs !== undefined && (
          <span
            className="ag-quiet tip"
            data-tip={`No report for ${formatDuration(child.quietMs)} — it may be on a long step`}
          >
            quiet {formatDuration(child.quietMs)}
          </span>
        )}
        <span className="ag-slot iflex-center">
          <span className="ag-meta">
            {child.running && <span className="ag-loader" aria-label="Working" />}
            {child.failed && (
              <span
                className="ag-failed tip"
                data-tip={failedTip([child])}
                aria-label="Sub-agent failed"
              >
                <Icon name="close" size={12} />
              </span>
            )}
          </span>
        </span>
      </div>
    </div>
  );
}
