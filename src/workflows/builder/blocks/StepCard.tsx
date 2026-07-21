// StepCard.tsx — one collapsed step card on the vertical canvas: agent identity,
// a 2-line goal preview, and summary chips (gate, budgets, comms). All editing
// happens in the inspector — clicking the card selects it there. Reused at the
// top level and inside parallel / loop / orchestrate containers.

import { Icon } from "../../../components/Icon";
import { GATE_MODES } from "../../data";
import { AgentAvatar } from "../AgentAvatar";
import type { BuilderCtx } from "../ctx";
import type { EStep } from "../model";

export function StepCard({
  step,
  ctx,
  indexLabel,
  canRemove,
  role = "step",
}: {
  step: EStep;
  ctx: BuilderCtx;
  indexLabel?: string;
  canRemove: boolean;
  /** A `child` step lives under an orchestrate/parallel container. */
  role?: "step" | "child";
}) {
  const a = ctx.resolve(step.agent);
  const gate = GATE_MODES.find((m) => m.id === step.gate.type) ?? GATE_MODES[0];
  const errors = ctx.errorsFor(step.nid);
  const selected = ctx.selectedNid === step.nid;
  const hasBudgets = !!step.budgets && Object.values(step.budgets).some((v) => v != null);

  return (
    <div
      className={`wb-step ${selected ? "sel" : ""} ${a ? "" : "unassigned"} ${errors ? "has-err" : ""}`}
      style={{ "--h": a?.hue ?? 250 } as React.CSSProperties}
      onClick={() => ctx.select(step.nid)}
    >
      <div className="wb-step-h">
        {indexLabel && <span className="wb-step-idx">{indexLabel}</span>}
        <button
          className="wb-step-agent"
          onClick={(e) => {
            e.stopPropagation();
            ctx.select(step.nid);
            ctx.openAgent(step.nid, "step", e);
          }}
        >
          {a ? (
            <AgentAvatar
              custom={a.custom}
              slug={a.providerId}
              short={a.short}
              hue={a.hue}
              size={28}
            />
          ) : (
            <span className="wb-step-mono empty">
              <Icon name="plus" size={12} />
            </span>
          )}
          <span className="wb-step-agent-text">
            <div className={`wb-an ${a ? "" : "empty"}`}>{a ? a.name : "Choose an agent"}</div>
            {a && <div className="wb-am">{a.custom ? `${a.baseLabel} · ${a.model}` : a.model}</div>}
          </span>
        </button>
        {canRemove && (
          <button
            className="tip wb-step-menu"
            data-tip-down
            data-tip={role === "child" ? "Remove" : "Remove step"}
            onClick={(e) => {
              e.stopPropagation();
              ctx.removeNode(step.nid);
            }}
          >
            <Icon name="close" />
          </button>
        )}
      </div>

      <div className={`wb-step-goal ${step.goal.trim() ? "" : "empty"}`}>
        {step.goal.trim() || "No instructions yet — select to add."}
      </div>

      <div className="wb-step-foot">
        <span className="wb-chip">
          <Icon name={gate.icon} /> Done on <b>{gate.short}</b>
        </span>
        {step.gate.type === "artifact" && step.gate.path.trim() && (
          <span className="wb-chip">
            <Icon name="file" /> {step.gate.path}
          </span>
        )}
        {hasBudgets && (
          <span className="wb-chip">
            <Icon name="clock" /> budgets
          </span>
        )}
        {step.comms.map((c) => (
          <span className="wb-chip" key={c}>
            {c}
          </span>
        ))}
        {errors && (
          <span className="wb-chip err">
            <Icon name="close" /> {errors.length} issue{errors.length === 1 ? "" : "s"}
          </span>
        )}
      </div>
    </div>
  );
}
