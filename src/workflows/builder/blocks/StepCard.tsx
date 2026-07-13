// StepCard.tsx — one step card: agent, goal, gate, comms caps, budgets. Reused
// at the top level and inside parallel / loop / orchestrate containers, so it
// takes only its step node + the shared editing ctx.

import { Icon } from "../../../components/Icon";
import { GATE_MODES, STEP_COMMS } from "../../data";
import type { CommsCap } from "../../spec";
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

  const toggleComms = (cap: CommsCap) => {
    const has = step.comms.includes(cap);
    ctx.patchStep(step.nid, {
      comms: has ? step.comms.filter((c) => c !== cap) : [...step.comms, cap],
    });
  };

  return (
    <div className={`wb-step ${errors ? "has-err" : ""}`}>
      <div className="wb-step-h">
        {indexLabel && <span className="wb-step-idx">{indexLabel}</span>}
        <button className="wb-step-agent" onClick={(e) => ctx.openAgent(step.nid, "step", e)}>
          {a ? (
            <AgentAvatar
              custom={a.custom}
              slug={a.providerId}
              short={a.short}
              hue={a.hue}
              size={26}
            />
          ) : (
            <span className="wb-step-mono empty">
              <Icon name="plus" size={12} />
            </span>
          )}
          <span className="wb-step-agent-text">
            <div className={`wb-an ${a ? "" : "empty"}`}>{a ? a.name : "Choose agent"}</div>
            {a && <div className="wb-am">{a.custom ? `${a.baseLabel} · ${a.model}` : a.model}</div>}
          </span>
        </button>
        {canRemove && (
          <button
            className="tip wb-step-menu"
            data-tip-down
            data-tip={role === "child" ? "Remove" : "Remove step"}
            onClick={() => ctx.removeNode(step.nid)}
          >
            <Icon name="close" />
          </button>
        )}
      </div>

      <textarea
        className="wb-step-goal"
        placeholder="What should this step accomplish?"
        value={step.goal}
        onChange={(e) => ctx.patchStep(step.nid, { goal: e.target.value })}
      />

      {step.gate.type === "artifact" && (
        <input
          className="ca-input wb-artifact"
          placeholder="Artifact path, e.g. PLAN.md"
          value={step.gate.path}
          onChange={(e) =>
            ctx.patchStep(step.nid, { gate: { type: "artifact", path: e.target.value } })
          }
        />
      )}

      <div className="wb-step-foot">
        <span className="wb-advance">
          <Icon name={gate.icon} />
          <span>
            Done on{" "}
            <button className="wb-adv-sel" onClick={(e) => ctx.openGate(step.nid, e)}>
              {gate.short}
            </button>
          </span>
        </span>
        <span className="grow" />
        <button
          className="wb-chip-btn tip"
          data-tip-down
          data-tip="Step budgets"
          onClick={(e) => ctx.openBudgets(step.nid, e)}
        >
          <Icon name="clock" size={11} />
          {(step.budgets?.turns_per_attempt ?? step.budgets?.turns) ? (
            <span>{step.budgets.turns_per_attempt ?? step.budgets.turns}t</span>
          ) : null}
        </button>
      </div>

      <div className="wb-comms">
        {STEP_COMMS.map((c) => (
          <button
            key={c.id}
            className={`wb-comm ${step.comms.includes(c.id) ? "on" : ""} tip`}
            data-tip-down
            data-tip={c.note}
            onClick={() => toggleComms(c.id)}
          >
            {c.label}
          </button>
        ))}
      </div>

      {errors && (
        <div className="wb-errs">
          {errors.map((msg) => (
            <span className="wb-err" key={msg}>
              <Icon name="close" size={9} /> {msg}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
