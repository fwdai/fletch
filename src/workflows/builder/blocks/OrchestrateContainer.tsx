// OrchestrateContainer.tsx — a lead agent coordinating children (spec §6.6/§10).
// Collapsed chrome on the canvas: the lead's identity + goal preview above the
// child grid, with summary chips for dynamic children / sub-workflows. The
// lead's goal, comms, and the dynamic/compose settings are edited in the
// inspector when the container is selected.

import { Icon } from "../../../components/Icon";
import { AgentAvatar } from "../AgentAvatar";
import type { BuilderCtx } from "../ctx";
import type { EOrchestrate } from "../model";
import { StepCard } from "./StepCard";

export function OrchestrateContainer({
  block,
  ctx,
  indexLabel,
}: {
  block: EOrchestrate;
  ctx: BuilderCtx;
  indexLabel?: string;
}) {
  const lead = ctx.resolve(block.agent);
  const child = ctx.resolve(block.children?.agent ?? null);
  const errors = ctx.errorsFor(block.nid);
  const selected = ctx.selectedNid === block.nid;

  return (
    <div
      className={`wb-cont wb-orch ${selected ? "sel" : ""} ${errors ? "has-err" : ""}`}
      onClick={() => ctx.select(block.nid)}
    >
      <div className="wb-cont-h">
        {indexLabel && <span className="wb-step-idx">{indexLabel}</span>}
        <span className="wb-cont-badge">
          <Icon name="combine" size={12} /> Orchestrate
        </span>
        <span className="wb-cont-sum">join {block.join}</span>
        {errors && (
          <span className="wb-chip err">
            <Icon name="close" /> {errors.length}
          </span>
        )}
        <button
          className="wb-step-menu tip"
          data-tip-down
          data-tip="Remove orchestrate"
          onClick={(e) => {
            e.stopPropagation();
            ctx.removeNode(block.nid);
          }}
        >
          <Icon name="close" />
        </button>
      </div>

      <div className="wb-orch-lead" style={{ "--h": lead?.hue ?? 250 } as React.CSSProperties}>
        <button
          className="wb-step-agent"
          onClick={(e) => {
            e.stopPropagation();
            ctx.select(block.nid);
            ctx.openAgent(block.nid, "orchestrator", e);
          }}
        >
          {lead ? (
            <AgentAvatar
              custom={lead.custom}
              slug={lead.providerId}
              short={lead.short}
              hue={lead.hue}
              size={28}
            />
          ) : (
            <span className="wb-step-mono empty">
              <Icon name="plus" size={12} />
            </span>
          )}
          <span className="wb-step-agent-text">
            <div className={`wb-an ${lead ? "" : "empty"}`}>
              {lead ? lead.name : "Choose an orchestrator"}
            </div>
            {lead && (
              <div className="wb-am">
                {lead.custom ? `${lead.baseLabel} · ${lead.model}` : lead.model}
              </div>
            )}
          </span>
        </button>
        <div className={`wb-step-goal ${block.goal.trim() ? "" : "empty"}`}>
          {block.goal.trim() || "No coordination brief yet — select to add."}
        </div>
        <div className="wb-step-foot">
          {block.comms.length > 0 && (
            <span className="wb-chip">children may {block.comms.join(" · ")}</span>
          )}
          {block.children && (
            <span className="wb-chip">
              <Icon name="sparkle" /> dynamic ×{block.children.max}
              {child ? ` · ${child.name}` : ""}
            </span>
          )}
          {block.compose && (
            <span className="wb-chip">
              <Icon name="combine" /> sub-workflows ×{block.compose.maxSubRuns}
            </span>
          )}
        </div>
      </div>

      {/* Clicks inside the body select the child card, not this container. */}
      <div className="wb-cont-body wb-branches" onClick={(e) => e.stopPropagation()}>
        {block.body.map((s, i) => (
          <StepCard key={s.nid} step={s} ctx={ctx} indexLabel={`${i + 1}`} canRemove role="child" />
        ))}
        <button
          className="wb-add sm"
          onClick={(e) => {
            e.stopPropagation();
            ctx.addStepToContainer(block.nid);
          }}
        >
          <span className="wb-add-ic">
            <Icon name="plus" />
          </span>
          <span className="wb-add-l">Add child</span>
        </button>
      </div>
    </div>
  );
}
