// inspector/index.tsx — the right-hand inspector: a sticky panel that edits the
// selected canvas node, or shows the workflow overview when nothing is selected.

import { Icon } from "../../../components/Icon";
import type { BuilderCtx } from "../ctx";
import type { EBlock, EditorState } from "../model";
import { LoopInspector, OrchestrateInspector, ParallelInspector } from "./ContainerInspectors";
import { OverviewInspector, type PromotePanel } from "./OverviewInspector";
import { StepInspector } from "./StepInspector";

function headerFor(selected: EBlock | null, state: EditorState, ctx: BuilderCtx) {
  if (!selected) return { eyebrow: "Overview", title: state.name.trim() || "Untitled workflow" };
  switch (selected.kind) {
    case "step": {
      const a = ctx.resolve(selected.agent);
      return { eyebrow: `Step · ${selected.stepId}`, title: a?.name ?? "Unassigned" };
    }
    case "parallel":
      return { eyebrow: "Block", title: "Parallel" };
    case "loop":
      return { eyebrow: "Block", title: "Loop" };
    case "orchestrate":
      return { eyebrow: "Block", title: "Orchestrate" };
  }
}

export function Inspector({
  state,
  selected,
  ctx,
  onField,
  formErrors,
  promote,
}: {
  state: EditorState;
  selected: EBlock | null;
  ctx: BuilderCtx;
  onField: (patch: Partial<EditorState>) => void;
  formErrors: string[];
  promote?: PromotePanel;
}) {
  const head = headerFor(selected, state, ctx);

  return (
    <aside className="wb-insp">
      <div className="wb-insp-h">
        <span className="wb-insp-eye">{head.eyebrow}</span>
        <span className="wb-insp-t">{head.title}</span>
        {selected && (
          <button
            className="wb-insp-close tip"
            data-tip-down
            data-tip="Back to overview"
            onClick={() => ctx.select(null)}
          >
            <Icon name="close" size={13} />
          </button>
        )}
      </div>
      <div className="wb-insp-body" key={selected?.nid ?? "overview"}>
        {!selected && (
          <OverviewInspector
            state={state}
            ctx={ctx}
            onField={onField}
            formErrors={formErrors}
            promote={promote}
          />
        )}
        {selected?.kind === "step" && <StepInspector step={selected} ctx={ctx} />}
        {selected?.kind === "parallel" && <ParallelInspector block={selected} ctx={ctx} />}
        {selected?.kind === "loop" && <LoopInspector block={selected} ctx={ctx} />}
        {selected?.kind === "orchestrate" && <OrchestrateInspector block={selected} ctx={ctx} />}
      </div>
    </aside>
  );
}
