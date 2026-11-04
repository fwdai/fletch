// OverviewInspector.tsx — the inspector when nothing is selected: the step
// count, a compact flow map, accent hue, and finalize actions.

import { Icon } from "../../../components/Icon";
import { AgentAvatar } from "../AgentAvatar";
import type { BuilderCtx } from "../ctx";
import type { EditorState, EStep } from "../model";
import { WF_HUES } from "../model";
import { ErrorStrip, Field } from "./bits";

function FlowRow({ index, step, ctx }: { index: number; step: EStep; ctx: BuilderCtx }) {
  const a = ctx.resolve(step.agent);
  return (
    <button className="wb-ov-step" onClick={() => ctx.select(step.nid)}>
      <span className="i">{String(index + 1).padStart(2, "0")}</span>
      {a ? (
        <AgentAvatar custom={a.custom} slug={a.providerId} short={a.short} hue={a.hue} size={22} />
      ) : (
        <span className="wb-step-mono empty sm">
          <Icon name="plus" size={10} />
        </span>
      )}
      <span className="n">{a?.name ?? "Unassigned"}</span>
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
  const steps = state.blocks.length;
  const finalize = state.finalize;

  return (
    <>
      <ErrorStrip errors={formErrors} />

      <div className="wb-stats">
        <div className="wb-stat">
          <div className="v">{steps}</div>
          <div className="k">step{steps === 1 ? "" : "s"}</div>
        </div>
      </div>

      <Field label="Flow">
        <div className="wb-ov-flow">
          {state.blocks.map((s, i) => (
            <FlowRow key={s.nid} index={i} step={s} ctx={ctx} />
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
              <label className="wb-inline-field">
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
