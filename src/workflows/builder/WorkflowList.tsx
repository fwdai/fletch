// WorkflowList.tsx — the list view: one card per definition with a compact
// block-chain preview, plus the "New workflow" entry point.

import { Fragment } from "react";
import { Icon } from "../../components/Icon";
import { SetHead } from "../../components/SettingsScreen/primitives";
import type { ModelMeta } from "../../data/modelCatalog/types";
import type { CustomAgent } from "../../storage/customAgents";
import { resolveAgent } from "../shared";
import type { AgentSpec, Block, Definition, Spec } from "../spec";
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

/** An alias's AgentSpec collapses to the picked identity: a custom agent (by its
 *  local id) or a bare base provider. */
function resolveAlias(
  spec: Spec,
  alias: string | undefined,
  agents: CustomAgent[],
  modelsByAgent: Record<string, ModelMeta[]>,
) {
  if (!alias) return null;
  const a: AgentSpec | undefined = spec.agents?.[alias];
  if (!a) return null;
  return resolveAgent(a.custom_agent ?? a.base, agents, modelsByAgent);
}

function StepChip({ label, agent }: { label: string; agent: ReturnType<typeof resolveAgent> }) {
  return (
    <span className="wf-mini-step">
      {agent ? (
        <AgentAvatar
          custom={agent.custom}
          slug={agent.providerId}
          short={agent.short}
          hue={agent.hue}
          size={16}
        />
      ) : (
        <span className="wf-mini-mono" style={{ "--h": 250 } as React.CSSProperties}>
          ?
        </span>
      )}
      {agent?.name ?? label}
    </span>
  );
}

function WorkflowMini({
  spec,
  agents,
  modelsByAgent,
}: {
  spec: Spec;
  agents: CustomAgent[];
  modelsByAgent: Record<string, ModelMeta[]>;
}) {
  const resolve = (alias?: string) => resolveAlias(spec, alias, agents, modelsByAgent);
  const blocks = spec.workflow ?? [];
  return (
    <div className="wf-mini">
      {blocks.map((b: Block, i) => (
        <Fragment key={i}>
          {i > 0 && (
            <span className="wf-mini-arrow">
              <Icon name="arrowR" />
            </span>
          )}
          {"step" in b && <StepChip label={b.step.id} agent={resolve(b.step.agent)} />}
          {"parallel" in b && (
            <span className="wf-mini-loop">
              <Icon name="layers" /> {b.parallel.steps.length} parallel
            </span>
          )}
          {"loop" in b && (
            <span className="wf-mini-loop">
              <Icon name="loop" /> loop ×{b.loop.max}
            </span>
          )}
          {"orchestrate" in b && (
            <span className="wf-mini-loop">
              <Icon name="combine" /> orchestrate
            </span>
          )}
        </Fragment>
      ))}
    </div>
  );
}

export function WorkflowList({
  definitions,
  loading,
  agents,
  modelsByAgent,
  onNew,
  onEdit,
  onDuplicate,
  onDelete,
}: {
  definitions: Definition[];
  loading: boolean;
  agents: CustomAgent[];
  modelsByAgent: Record<string, ModelMeta[]>;
  onNew: () => void;
  onEdit: (d: Definition) => void;
  onDuplicate: (d: Definition) => void;
  onDelete: (id: string) => void;
}) {
  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Workflows"
        title="Workflows"
        desc="Chain agents into a repeatable pipeline. Each step hands its work to the next on the same git branch — so the checkout itself is the shared context. Define once here; launch on any task."
      />

      <div className="set-list-head">
        <span className="sl-count">
          {definitions.length} workflow{definitions.length === 1 ? "" : "s"}
        </span>
        <button className="btn-t primary" onClick={onNew}>
          <Icon name="plus" size={13} /> New workflow
        </button>
      </div>

      <div className="wf-list">
        {definitions.map((d) => (
          <div key={d.id} className="wf-card" onClick={() => onEdit(d)}>
            <div className="wf-card-h">
              <div style={{ flex: 1, minWidth: 0 }}>
                <div className="wf-name">{d.name}</div>
                <div className="wf-desc">{d.description}</div>
              </div>
              <div className="wf-meta">
                <span>{d.run_count ?? 0} runs</span>
                <span>·</span>
                <span>edited {timeAgo(d.updated_at)}</span>
              </div>
              <div className="wf-acts" onClick={(e) => e.stopPropagation()}>
                <button
                  className="btn-i sm tip"
                  data-tip-down
                  data-tip="Duplicate"
                  onClick={() => onDuplicate(d)}
                >
                  <Icon name="copy" />
                </button>
                <button
                  className="btn-i sm tip"
                  data-tip-down
                  data-tip="Delete"
                  onClick={() => onDelete(d.id)}
                >
                  <Icon name="trash" />
                </button>
              </div>
            </div>
            <WorkflowMini spec={d.spec} agents={agents} modelsByAgent={modelsByAgent} />
          </div>
        ))}
        {!loading && definitions.length === 0 && (
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
