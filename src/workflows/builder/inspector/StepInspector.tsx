// StepInspector.tsx — edits the selected step: agent, instructions, and the gate
// that marks it done.

import { Icon } from "../../../components/Icon";
import { GATE_MODES } from "../../data";
import type { BuilderCtx } from "../ctx";
import type { EStep } from "../model";
import { AgentButton, ErrorStrip, Field } from "./bits";

export function StepInspector({ step, ctx }: { step: EStep; ctx: BuilderCtx }) {
  const a = ctx.resolve(step.agent);

  return (
    <>
      <ErrorStrip errors={ctx.errorsFor(step.nid)} />

      <Field label="Agent" required>
        <AgentButton
          agent={a}
          placeholder="Choose an agent"
          onClick={(e) => ctx.openAgent(step.nid, e)}
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
              onClick={() => ctx.patchStep(step.nid, { gate: { type: m.id } })}
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
      </Field>
    </>
  );
}
