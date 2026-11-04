// WorkflowBuilder.tsx — the step-list editor (spec §14.1): a vertical pipeline
// canvas on the left, a sticky inspector on the right editing the selected step
// (or the workflow overview). A shared `ctx` lets any card dispatch an edit,
// select itself, or open the agent picker. Validation from `model.ts` renders
// inline and gates the save. Persistence is the caller's job (Save hands back
// the editor state).

import { useEffect, useMemo, useState } from "react";
import { Icon } from "../../components/Icon";
import type { ModelMeta } from "../../data/modelCatalog/types";
import type { CustomAgent } from "../../storage/customAgents";
import { resolveAlias } from "../shared";
import { BlockSequence } from "./blocks/BlockSequence";
import type { BuilderCtx, Pop, PopRect } from "./ctx";
import {
  editorStepIds,
  findStep,
  insertStep,
  patchStep,
  removeStep,
  setAgent,
  setField,
} from "./edits";
import { useDismissOnViewportChange } from "./hooks";
import { Inspector } from "./inspector";
import type { EditorState, NodeId } from "./model";
import { newStep, validateEditor } from "./model";
import { AgentPick } from "./pickers";

function rectFrom(e: React.MouseEvent): PopRect {
  const r = e.currentTarget.getBoundingClientRect();
  return { top: r.bottom + 6, left: r.left, right: r.right, bottom: r.bottom };
}

export function WorkflowBuilder({
  initial,
  isNew,
  agents,
  modelsByAgent,
  saving,
  saveError,
  onCancel,
  onSave,
}: {
  initial: EditorState;
  isNew: boolean;
  agents: CustomAgent[];
  modelsByAgent: Record<string, ModelMeta[]>;
  saving: boolean;
  saveError: string | null;
  onCancel: () => void;
  onSave: (state: EditorState) => void;
}) {
  const [state, setState] = useState<EditorState>(initial);
  const [sel, setSel] = useState<NodeId | null>(null);
  const [pop, setPop] = useState<Pop | null>(null);
  const closePop = () => setPop(null);
  useDismissOnViewportChange(!!pop, closePop);

  const validation = useMemo(() => validateEditor(state), [state]);

  // The inspector renders as a full-height right-hand drawer pinned outside the
  // settings content column. Flag the shell while the builder is mounted so the
  // scroll area reserves room for it (see `.set-main` rule in workflows.css).
  useEffect(() => {
    document.body.classList.add("wb-drawer-open");
    return () => document.body.classList.remove("wb-drawer-open");
  }, []);

  // A deleted step can't stay selected — fall back to the overview.
  const selected = sel ? findStep(state, sel) : null;
  useEffect(() => {
    if (sel && !findStep(state, sel)) setSel(null);
  }, [state, sel]);

  const ctx: BuilderCtx = useMemo(
    () => ({
      // `id` is an alias into `state.agents`; `resolveAlias` maps it to the
      // underlying custom-agent id or base provider before rendering.
      resolve: (id) => resolveAlias(state.agents, id ?? undefined, agents, modelsByAgent),
      errorsFor: (nid) => validation.byNode[nid],
      selectedNid: sel,
      select: setSel,
      patchStep: (nid, patch) => setState((s) => patchStep(s, nid, patch)),
      removeStep: (nid) => setState((s) => removeStep(s, nid)),
      // An add builds the step eagerly (a fixed nid to select; the global id
      // counter keeps step ids unique) but inserts with a functional update, so
      // rapid successive adds merge instead of overwriting each other.
      addStep: (index) => {
        const step = newStep(editorStepIds(state));
        setState((s) => insertStep(s, step, index));
        setSel(step.nid);
      },
      openAgent: (nid, e) => setPop({ type: "agent", nid, rect: rectFrom(e) }),
    }),
    // validation is derived from state, so ctx refreshes on every edit — the
    // closures over `state` above stay current.
    [agents, modelsByAgent, validation, state, sel],
  );

  const onField = (patch: Partial<EditorState>) => setState((s) => setField(s, patch));

  return (
    <div className="set-pane">
      <div className="wb">
        <div className="set-eyebrow mono text-xs wb-section-eye">Settings · Workflows</div>
        <button className="ca-ed-back iflex-center text-sm" onClick={onCancel}>
          <Icon name="chevL" size={13} /> All workflows
        </button>

        <div className="wb-body">
          <div className="wb-canvas" style={{ "--h": state.hue } as React.CSSProperties}>
            <div className="wb-head">
              <input
                className="wb-name"
                placeholder="Name this workflow…"
                value={state.name}
                autoFocus={isNew}
                onChange={(e) => onField({ name: e.target.value })}
              />
              <textarea
                className="wb-desc"
                rows={2}
                placeholder="What is this pipeline for? Each step hands off to the next."
                value={state.description}
                onChange={(e) => onField({ description: e.target.value })}
              />
            </div>

            <BlockSequence steps={state.blocks} ctx={ctx} />
          </div>

          <Inspector
            state={state}
            selected={selected}
            ctx={ctx}
            onField={onField}
            formErrors={validation.form}
          />
        </div>

        <div className="wb-foot">
          <div className="wb-summary">
            {validation.ok ? (
              <>
                Ready to save · <b>{state.blocks.length}</b> step
                {state.blocks.length === 1 ? "" : "s"}
              </>
            ) : (
              <span className="wb-summary-err">
                <Icon name="close" size={11} />{" "}
                {validation.form[0] ??
                  `${Object.keys(validation.byNode).length} step(s) need attention`}
              </span>
            )}
            {saveError && <span className="wb-summary-err"> · {saveError}</span>}
          </div>
          <span className="grow" />
          <button className="btn-t ghost" onClick={onCancel}>
            Cancel
          </button>
          <button
            className="btn-t primary"
            disabled={!validation.ok || saving}
            style={!validation.ok || saving ? { opacity: 0.5 } : undefined}
            onClick={() => validation.ok && onSave(state)}
          >
            <Icon name="check" size={13} />{" "}
            {saving ? "Saving…" : isNew ? "Create workflow" : "Save workflow"}
          </button>
        </div>
      </div>

      {pop && <div style={{ position: "fixed", inset: 0, zIndex: 55 }} onClick={closePop} />}
      {pop?.type === "agent" && (
        <AgentPick
          rect={pop.rect}
          agents={agents}
          onPick={(id) => {
            setState((s) => setAgent(s, pop.nid, id, agents));
            closePop();
          }}
        />
      )}
    </div>
  );
}
