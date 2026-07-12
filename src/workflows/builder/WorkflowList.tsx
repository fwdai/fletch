// WorkflowList.tsx — the list view: one card per workflow with a compact
// step-chain preview, plus the "New workflow" entry point.

import { Fragment } from "react";
import { Icon } from "../../components/Icon";
import { SetHead } from "../../components/SettingsScreen/primitives";
import type { ModelMeta } from "../../data/modelCatalog/types";
import type { CustomAgent } from "../../storage/customAgents";
import type { AgentResolver } from "../shared";
import { resolveAgent } from "../shared";
import type { Workflow } from "../storage";
import { AgentAvatar } from "./AgentAvatar";

function timeAgo(ms: number): string {
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
}

function WorkflowMini({ workflow, resolve }: { workflow: Workflow; resolve: AgentResolver }) {
  const loopStep = workflow.steps.find((s) => s.loop);
  return (
    <div className="wf-mini">
      {workflow.steps.map((s, i) => {
        const a = resolve(s.agent);
        return (
          <Fragment key={s.id}>
            {i > 0 && (
              <span className="wf-mini-arrow">
                <Icon name="arrowR" />
              </span>
            )}
            <span className="wf-mini-step">
              {a ? (
                <AgentAvatar
                  custom={a.custom}
                  slug={a.providerId}
                  short={a.short}
                  hue={a.hue}
                  size={16}
                />
              ) : (
                <span className="wf-mini-mono" style={{ "--h": 250 } as React.CSSProperties}>
                  ?
                </span>
              )}
              {a?.name || "Unassigned"}
            </span>
          </Fragment>
        );
      })}
      {loopStep && (
        <span className="wf-mini-loop">
          <Icon name="loop" /> until {loopStep.loop!.when?.split(" ")[0] || "done"}
        </span>
      )}
    </div>
  );
}

export function WorkflowList({
  workflows,
  loading,
  agents,
  modelsByAgent,
  onNew,
  onEdit,
  onDuplicate,
  onDelete,
}: {
  workflows: Workflow[];
  loading: boolean;
  agents: CustomAgent[];
  modelsByAgent: Record<string, ModelMeta[]>;
  onNew: () => void;
  onEdit: (w: Workflow) => void;
  onDuplicate: (w: Workflow) => void;
  onDelete: (id: string) => void;
}) {
  const resolve = (id: string | null) => resolveAgent(id, agents, modelsByAgent);

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Workflows"
        title="Workflows"
        desc="Chain agents into a repeatable pipeline. Each step hands its work to the next on the same git branch — so the checkout itself is the shared context. Define once here; launch on any task."
      />

      <div className="set-list-head">
        <span className="sl-count">
          {workflows.length} workflow{workflows.length === 1 ? "" : "s"}
        </span>
        <button className="btn-t primary" onClick={onNew}>
          <Icon name="plus" size={13} /> New workflow
        </button>
      </div>

      <div className="wf-list">
        {workflows.map((w) => (
          <div key={w.id} className="wf-card" onClick={() => onEdit(w)}>
            <div className="wf-card-h">
              <div style={{ flex: 1, minWidth: 0 }}>
                <div className="wf-name">{w.name}</div>
                <div className="wf-desc">{w.description}</div>
              </div>
              <div className="wf-meta">
                <span>{w.run_count ?? 0} runs</span>
                <span>·</span>
                <span>edited {timeAgo(w.updated_at)}</span>
              </div>
              <div className="wf-acts" onClick={(e) => e.stopPropagation()}>
                <button
                  className="btn-i sm tip"
                  data-tip-down
                  data-tip="Duplicate"
                  onClick={() => onDuplicate(w)}
                >
                  <Icon name="copy" />
                </button>
                <button
                  className="btn-i sm tip"
                  data-tip-down
                  data-tip="Delete"
                  onClick={() => onDelete(w.id)}
                >
                  <Icon name="trash" />
                </button>
              </div>
            </div>
            <WorkflowMini workflow={w} resolve={resolve} />
          </div>
        ))}
        {!loading && workflows.length === 0 && (
          <button className="wb-add" style={{ width: "100%", minHeight: 120 }} onClick={onNew}>
            <span className="wb-add-ic">
              <Icon name="plus" />
            </span>
            <span className="wb-add-l">Create your first workflow</span>
          </button>
        )}
      </div>
    </div>
  );
}
