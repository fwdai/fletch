// inspector/index.tsx — the right-hand inspector: a sticky panel that edits the
// selected step, or shows the workflow overview when nothing is selected.

import { Icon } from "../../../components/Icon";
import type { BuilderCtx } from "../ctx";
import type { EditorState, EStep } from "../model";
import { OverviewInspector } from "./OverviewInspector";
import { StepInspector } from "./StepInspector";

export function Inspector({
  state,
  selected,
  ctx,
  onField,
  formErrors,
}: {
  state: EditorState;
  selected: EStep | null;
  ctx: BuilderCtx;
  onField: (patch: Partial<EditorState>) => void;
  formErrors: string[];
}) {
  const head = selected
    ? {
        eyebrow: `Step · ${selected.stepId}`,
        title: ctx.resolve(selected.agent)?.name ?? "Unassigned",
      }
    : { eyebrow: "Overview", title: state.name.trim() || "Untitled workflow" };

  return (
    <aside className="wb-insp">
      <div className="wb-insp-h">
        <span className="wb-insp-eye">{head.eyebrow}</span>
        <span className="wb-insp-t">{head.title}</span>
        {selected && (
          <button
            className="wb-insp-close tip"
            data-tip="Back to overview"
            onClick={() => ctx.select(null)}
          >
            <Icon name="close" size={13} />
          </button>
        )}
      </div>
      <div className="wb-insp-body" key={selected?.nid ?? "overview"}>
        {selected ? (
          <StepInspector step={selected} ctx={ctx} />
        ) : (
          <OverviewInspector state={state} ctx={ctx} onField={onField} formErrors={formErrors} />
        )}
      </div>
    </aside>
  );
}
