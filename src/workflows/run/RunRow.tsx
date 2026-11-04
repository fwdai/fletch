// run/RunRow.tsx — a workflow run as a sidebar row. Same skeleton and status
// vocabulary as an agent row (AgentRow) so it reads as native, with one
// deliberate difference: a persistent accent-tinted workflow tile where an
// agent row starts with its name. The tile doubles as the expander for the
// run's live step agents — the workflow mark never disappears, the chevron is
// revealed on hover (progressive disclosure, constant identity).

import { type KeyboardEvent, type MouseEvent, useState } from "react";
import { api, type WfRun } from "../../api";
import { Icon } from "../../components/Icon";
import { StepAgentRow } from "../../components/Sidebar/StepAgentRow";
import { deriveStepChildren } from "../../components/Sidebar/stepChildren";
import { Badge } from "../../components/ui/Badge";
import { useAppStore } from "../../store";
import { firstLine, formatAge } from "../../util/format";
import { useMinuteClock } from "../../util/hooks";
import { AgentAvatar } from "../builder/AgentAvatar";
import { resolveAlias } from "../shared";
import type { Spec } from "../spec";
import { flattenSteps } from "./RunView/flatten";
import { useRunAgents } from "./useRunAgents";

export function RunRow({
  run,
  selected,
  onSelect,
}: {
  run: WfRun;
  selected: boolean;
  onSelect: () => void;
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
  const deletable = run.status === "done" || run.status === "failed" || run.status === "canceled";
  const [confirmDelete, setConfirmDelete] = useState(false);

  // Expandable step-agent children, only while the run is live: a finished
  // run's step agents are archived, and a pending run hasn't spawned any yet.
  // Default expanded; `userExpanded` (null until the user toggles) overrides
  // that. Agents are fetched only while expanded, so collapsed runs cost nothing.
  const canExpand = working;
  const [userExpanded, setUserExpanded] = useState<boolean | null>(null);
  const expanded = canExpand && (userExpanded ?? true);
  const stepChildren = deriveStepChildren(useRunAgents(run.id, expanded));
  // Same left-spine vocabulary as an agent row: live → green, failed → danger,
  // everything else (pending/done/canceled) → the faint idle grey.
  const railClass = working ? "run" : run.status === "failed" ? "err" : "idle";
  const age = formatAge(new Date(run.created_at).toISOString(), now);

  // The flow's lead (first) agent — a representative chip; the workflow tile is
  // what marks the row as a run. Resolved from the launch-snapshot spec.
  const spec = run.spec as Spec | null;
  const first = flattenSteps(spec)[0];
  const a = first
    ? resolveAlias(spec?.agents, first.agentAlias, customAgents, modelsByAgent)
    : null;

  // The status meta is wider than the reserved 21px slot when it carries a text
  // badge. Every status offers Stop or Delete, so the meta always cross-fades to
  // the hover actions.
  const wideMeta = run.status === "failed";

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

  const onToggleExpand = (e: MouseEvent) => {
    e.stopPropagation();
    setUserExpanded(!expanded);
  };

  return (
    <>
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
        onMouseLeave={() => setConfirmDelete(false)}
      >
        <span className={`ag-rail ${railClass}`} />
        <div className="agent-row flex-center">
          {canExpand ? (
            <button
              className={`ag-wf-tile expandable ${expanded ? "open" : ""}`}
              onClick={onToggleExpand}
              aria-label={expanded ? "Collapse steps" : "Expand steps"}
              aria-expanded={expanded}
            >
              <Icon name="combine" size={11} className="wf-glyph" />
              <Icon name="chevR" size={11} className="wf-chev" />
            </button>
          ) : (
            <span className="ag-wf-tile tip" data-tip="Workflow run">
              <Icon name="combine" size={11} className="wf-glyph" />
            </span>
          )}
          <span className={`ag-name ag-name-run ${working ? "shimmer" : ""}`}>{run.name}</span>
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
          <span className="ag-slot iflex-center">
            <span className={`ag-meta ${wideMeta ? "wide" : ""}`}>
              {working && <span className="ag-loader" aria-label="Working" />}
              {run.status === "failed" && <Badge variant="err">failed</Badge>}
              {run.status === "done" && (
                <span className="ag-run-done tip" data-tip="Completed" aria-label="Completed">
                  <Icon name="check" size={12} />
                </span>
              )}
            </span>
            <span className="ag-actions">
              {stoppable && (
                <button
                  className="ag-act iflex-center tip"
                  data-tip="Stop"
                  onClick={(e) => void onStop(e)}
                  aria-label="Stop"
                >
                  <Icon name="stop" size={11} />
                </button>
              )}
              {deletable && (
                <button
                  className={`ag-act iflex-center tip ${confirmDelete ? "confirm-del" : ""}`}
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
        <div className="agent-sub flex-center">
          <span className="a-task">{firstLine(run.task || "Workflow run")}</span>
          <span className="a-time">{age}</span>
        </div>
      </div>
      {expanded && stepChildren.length > 0 && (
        <div className="run-steps">
          {stepChildren.map((child) => (
            <StepAgentRow key={child.agent.id} runId={run.id} child={child} />
          ))}
        </div>
      )}
    </>
  );
}
