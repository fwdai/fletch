// StepInspector.tsx — edits the selected step: agent, instructions, done-gate
// (the full §9 option list, inline instead of the old popover), budgets, comms.

import { Icon } from "../../../components/Icon";
import { GATE_MODES, type GateKind, STEP_COMMS } from "../../data";
import type { CommsCap, Gate } from "../../spec";
import { ContainerErrors } from "../blocks/ContainerErrors";
import type { BuilderCtx } from "../ctx";
import type { EStep } from "../model";
import { AgentButton, BudgetFields, Field, STEP_BUDGET_FIELDS } from "./bits";

export function StepInspector({ step, ctx }: { step: EStep; ctx: BuilderCtx }) {
  const a = ctx.resolve(step.agent);
  const errors = ctx.errorsFor(step.nid);

  const pickGate = (kind: GateKind) => {
    const cur = step.gate;
    const gate: Gate =
      kind === "artifact"
        ? { type: "artifact", path: cur.type === "artifact" ? cur.path : "" }
        : // Re-picking approval preserves any existing `require: [tests]`.
          kind === "approval"
          ? { type: "approval", require: cur.type === "approval" ? cur.require : undefined }
          : { type: kind };
    ctx.patchStep(step.nid, { gate });
  };

  const toggleComms = (cap: CommsCap) => {
    const has = step.comms.includes(cap);
    ctx.patchStep(step.nid, {
      comms: has ? step.comms.filter((c) => c !== cap) : [...step.comms, cap],
    });
  };

  const requireTests = step.gate.type === "approval" && (step.gate.require?.includes("tests") ?? false);

  return (
    <>
      <ContainerErrors errors={errors} />

      <Field label="Agent" required>
        <AgentButton
          agent={a}
          placeholder="Choose an agent"
          onClick={(e) => ctx.openAgent(step.nid, "step", e)}
        />
      </Field>

      <Field label="Instructions" hint="what this step should accomplish">
        <textarea
          className="wb-insp-textarea"
          value={step.goal}
          placeholder="e.g. Explore the codebase and write a focused implementation plan to PLAN.md. Don't write feature code."
          onChange={(e) => ctx.patchStep(step.nid, { goal: e.target.value })}
        />
      </Field>

      <Field label="This step is done when…">
        <div className="wb-opts">
          {GATE_MODES.map((m) => (
            <button
              key={m.id}
              className={`wb-opt ${m.id === step.gate.type ? "on" : ""}`}
              onClick={() => pickGate(m.id)}
            >
              <span className="wb-opt-ic">
                <Icon name={m.icon} />
              </span>
              <span className="wb-opt-l">
                <span className="wb-opt-t">
                  {m.label}
                  {m.id === step.gate.type && <Icon className="ck" name="check" size={13} />}
                </span>
                <span className="wb-opt-s">{m.note}</span>
              </span>
            </button>
          ))}
        </div>
        {step.gate.type === "artifact" && (
          <input
            className="ca-input"
            style={{ marginTop: 9 }}
            value={step.gate.path}
            placeholder="File to wait for, e.g. PLAN.md"
            onChange={(e) =>
              ctx.patchStep(step.nid, { gate: { type: "artifact", path: e.target.value } })
            }
          />
        )}
        {step.gate.type === "approval" && (
          // One extra prerequisite the approval gate can require first — the human
          // pause stays unreachable until the project's tests pass (spec §9).
          <label className="wb-toggle" style={{ marginTop: 9 }}>
            <input
              type="checkbox"
              checked={requireTests}
              onChange={(e) =>
                ctx.patchStep(step.nid, {
                  gate: { type: "approval", require: e.target.checked ? ["tests"] : undefined },
                })
              }
            />
            Require passing tests first
          </label>
        )}
      </Field>

      <Field label="Budgets" hint="pauses when exceeded">
        <BudgetFields
          fields={STEP_BUDGET_FIELDS}
          value={step.budgets}
          onChange={(next) => ctx.patchStep(step.nid, { budgets: next })}
        />
      </Field>

      <Field label="This step may">
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
      </Field>
    </>
  );
}
