// OverviewInspector.tsx — the inspector when nothing is selected: run-level
// stats, a compact flow map, accent hue, run budgets, and finalize actions.

import { Icon } from "../../../components/Icon";
import { AgentAvatar } from "../AgentAvatar";
import { ContainerErrors } from "../blocks/ContainerErrors";
import type { BuilderCtx } from "../ctx";
import type { EBlock, EditorState } from "../model";
import { WF_HUES } from "../model";
import { BudgetFields, Field, RUN_BUDGET_FIELDS } from "./bits";

function countSteps(blocks: EBlock[]): number {
  let n = 0;
  for (const b of blocks) {
    if (b.kind === "step") n += 1;
    else if (b.kind === "parallel") n += b.steps.length;
    else if (b.kind === "loop") n += countSteps(b.body);
    else n += b.body.length;
  }
  return n;
}

/** Loops anywhere in the tree — imported specs may nest them in loop bodies. */
function countLoops(blocks: EBlock[]): number {
  let n = 0;
  for (const b of blocks) {
    if (b.kind === "loop") n += 1 + countLoops(b.body);
  }
  return n;
}

function FlowRow({ index, block, ctx }: { index: number; block: EBlock; ctx: BuilderCtx }) {
  const idx = String(index + 1).padStart(2, "0");
  if (block.kind === "step") {
    const a = ctx.resolve(block.agent);
    return (
      <button className="wb-ov-step" onClick={() => ctx.select(block.nid)}>
        <span className="i">{idx}</span>
        {a ? (
          <AgentAvatar
            custom={a.custom}
            slug={a.providerId}
            short={a.short}
            hue={a.hue}
            size={22}
          />
        ) : (
          <span className="wb-step-mono empty sm">
            <Icon name="plus" size={10} />
          </span>
        )}
        <span className="n">{a?.name ?? "Unassigned"}</span>
      </button>
    );
  }
  const meta =
    block.kind === "parallel"
      ? { icon: "layers" as const, label: `Parallel · ${block.steps.length} branches` }
      : block.kind === "loop"
        ? { icon: "loop" as const, label: `Loop · up to ${block.max}×` }
        : { icon: "combine" as const, label: "Orchestrate" };
  return (
    <button className="wb-ov-step" onClick={() => ctx.select(block.nid)}>
      <span className="i">{idx}</span>
      <span className="wb-ov-cont-ic">
        <Icon name={meta.icon} size={12} />
      </span>
      <span className="n">{meta.label}</span>
    </button>
  );
}

export function OverviewInspector({
  state,
  ctx,
  onField,
  formErrors,
}: {
  state: EditorState;
  ctx: BuilderCtx;
  onField: (patch: Partial<EditorState>) => void;
  formErrors: string[];
}) {
  const steps = countSteps(state.blocks);
  const loops = countLoops(state.blocks);
  const finalize = state.finalize;

  return (
    <>
      <ContainerErrors errors={formErrors.length ? formErrors : undefined} />

      <div className="wb-stats">
        <div className="wb-stat">
          <div className="v">{state.blocks.length}</div>
          <div className="k">block{state.blocks.length === 1 ? "" : "s"}</div>
        </div>
        <div className="wb-stat">
          <div className="v">{steps}</div>
          <div className="k">step{steps === 1 ? "" : "s"}</div>
        </div>
        <div className="wb-stat">
          <div className="v">{loops}</div>
          <div className="k">loop{loops === 1 ? "" : "s"}</div>
        </div>
      </div>

      <Field label="Flow">
        <div className="wb-ov-flow">
          {state.blocks.map((b, i) => (
            <FlowRow key={b.nid} index={i} block={b} ctx={ctx} />
          ))}
        </div>
      </Field>

      <Field label="Accent">
        <div className="wb-hues">
          {WF_HUES.map((h) => (
            <button
              key={h}
              className={`wb-hue ${state.hue === h ? "on" : ""}`}
              style={{ "--h": h } as React.CSSProperties}
              onClick={() => onField({ hue: h })}
            />
          ))}
        </div>
      </Field>

      <Field label="Run budgets" hint="pauses when exceeded">
        <BudgetFields
          fields={RUN_BUDGET_FIELDS}
          value={state.budgets}
          onChange={(next) => onField({ budgets: next })}
        />
      </Field>

      <Field label="When the run finishes">
        <div className="wb-box">
          <label className="wb-toggle">
            <input
              type="checkbox"
              checked={!!finalize?.push}
              onChange={(e) =>
                onField({
                  finalize: {
                    push: e.target.checked,
                    open_pr: finalize?.open_pr ?? false,
                    pr_base: finalize?.pr_base,
                  },
                })
              }
            />
            Push the branch
          </label>
          <label className="wb-toggle">
            <input
              type="checkbox"
              checked={!!finalize?.open_pr}
              onChange={(e) =>
                onField({
                  finalize: {
                    push: finalize?.push ?? false,
                    open_pr: e.target.checked,
                    pr_base: finalize?.pr_base,
                  },
                })
              }
            />
            Open a PR
          </label>
          {finalize?.open_pr && (
            <div className="wb-box-cfg">
              <label className="wb-budget-field">
                <span>PR base branch</span>
                <input
                  className="ca-input sm"
                  placeholder="main"
                  value={finalize.pr_base ?? ""}
                  onChange={(e) =>
                    onField({
                      finalize: {
                        push: finalize.push,
                        open_pr: finalize.open_pr,
                        pr_base: e.target.value.trim() || undefined,
                      },
                    })
                  }
                />
              </label>
            </div>
          )}
        </div>
      </Field>

      <div className="wb-field-note">
        Select a step on the canvas to edit its agent, instructions, and hand-off rules.
      </div>
    </>
  );
}
