import type { KeyboardEvent } from "react";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { formatDuration } from "@/util/format";
import type { AgentPr } from "@/util/prState";
import { PrPill } from "./PrPill";
import { failedTip, type SubagentChild } from "./subagentChildren";

/** One of an agent's backgrounded sub-agents as a sidebar child of its
 *  AgentRow — the same indented, quieter row a workflow run's step agents get
 *  (StepAgentRow), speaking the same rail / shimmer / loader vocabulary.
 *  Clicking opens the sub-agent's thread in the center pane (the agent's
 *  conversation is one "back" away). Reads active while that thread — or a
 *  thread nested under it — is open. The task's lifecycle is Claude's, so the
 *  row carries no actions. `prs` are the agent's PRs this sub-agent opened
 *  (`prOrigins`): its own pill, while the agent's row keeps the aggregate. */
export function SubagentRow({
  agentId,
  child,
  prs,
}: {
  agentId: string;
  child: SubagentChild;
  prs: readonly AgentPr[];
}) {
  const openSubagentThread = useAppStore((s) => s.openSubagentThread);
  const active = useAppStore(
    (s) => s.openThread?.agentId === agentId && s.openThread.path[0] === child.task.toolUseId,
  );
  const onSelect = () => openSubagentThread(agentId, [child.task.toolUseId]);

  return (
    <div
      className={`agent run-step subagent no-actions ${active ? "active" : ""}`}
      role="button"
      tabIndex={0}
      aria-current={active ? "page" : undefined}
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
        <PrPill prs={prs} />
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
