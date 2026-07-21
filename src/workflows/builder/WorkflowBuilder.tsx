// WorkflowBuilder.tsx — the block-tree editor (spec §14.1), v2 layout: a
// vertical pipeline canvas on the left, a sticky inspector on the right editing
// the selected node (or the workflow overview). A shared `ctx` lets any nested
// card dispatch an edit, select itself, or open the agent picker. Validation
// from `model.ts` renders inline and gates the save. Persistence is the
// caller's job (Save hands back the editor state).

import { useEffect, useMemo, useState } from "react";
import { Icon } from "../../components/Icon";
import type { ModelMeta } from "../../data/modelCatalog/types";
import type { CustomAgent } from "../../storage/customAgents";
import { resolveAlias } from "../shared";
import type { Spec } from "../spec";
import { BlockSequence } from "./blocks/BlockSequence";
import type { BuilderCtx, Pop, PopRect } from "./ctx";
import {
  type AgentRole,
  addBlockOfType,
  addStepToContainer,
  findNode,
  patchBlock,
  patchStep,
  removeNode,
  setAgent,
  setField,
} from "./edits";
import { useDismissOnViewportChange } from "./hooks";
import { Inspector } from "./inspector";
import type { EditorState, NodeId } from "./model";
import { toSpec, validateEditor } from "./model";
import { AgentPick } from "./pickers";

/** Launch context threaded in when the builder was opened by "promote to
 *  workflow": the run's fork point + brief ride alongside the definition so the
 *  user can launch straight from the builder, no re-typing. */
export interface PromoteLaunch {
  /** Prefilled run task (the promoted session's brief). */
  taskSeed: string;
  /** Human label for the fork point (short SHA or branch). */
  baseLabel: string;
  launching: boolean;
  launchError: string | null;
  onLaunch: (spec: Spec, task: string) => void;
}

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
  promote,
}: {
  initial: EditorState;
  isNew: boolean;
  agents: CustomAgent[];
  modelsByAgent: Record<string, ModelMeta[]>;
  saving: boolean;
  saveError: string | null;
  onCancel: () => void;
  onSave: (state: EditorState) => void;
  promote?: PromoteLaunch;
}) {
  const [state, setState] = useState<EditorState>(initial);
  const [sel, setSel] = useState<NodeId | null>(null);
  const [launchTask, setLaunchTask] = useState(promote?.taskSeed ?? "");
  const [pop, setPop] = useState<Pop | null>(null);
  const closePop = () => setPop(null);
  useDismissOnViewportChange(!!pop, closePop);

  const validation = useMemo(() => validateEditor(state), [state]);

  // A deleted node can't stay selected — fall back to the overview.
  const selected = sel ? findNode(state, sel) : null;
  useEffect(() => {
    if (sel && !findNode(state, sel)) setSel(null);
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
      patchBlock: (nid, patch) => setState((s) => patchBlock(s, nid, patch)),
      removeNode: (nid) => setState((s) => removeNode(s, nid)),
      addStepToContainer: (nid) => {
        const r = addStepToContainer(state, nid);
        setState(r.state);
        setSel(r.nid);
      },
      addBlock: (seqNid, type, index) => {
        const r = addBlockOfType(state, seqNid, type, index);
        setState(r.state);
        setSel(r.nid);
      },
      openAgent: (nid, role: AgentRole, e) =>
        setPop({ type: "agent", nid, role, rect: rectFrom(e) }),
    }),
    // validation is derived from state, so ctx refreshes on every edit — the
    // closures over `state` above stay current.
    [agents, modelsByAgent, validation, state, sel],
  );

  const onField = (patch: Partial<EditorState>) => setState((s) => setField(s, patch));

  return (
    <div className="set-pane">
      <div className="wb">
        <button className="ca-ed-back" onClick={onCancel}>
          <Icon name="chevL" /> All workflows
        </button>

        <div className="wb-body">
          <div className="wb-canvas" style={{ "--h": state.hue } as React.CSSProperties}>
            <div className="wb-head">
              <div className="wb-eyebrow">Workflow pipeline</div>
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

            <BlockSequence blocks={state.blocks} seqNid={null} ctx={ctx} />
          </div>

          <Inspector
            state={state}
            selected={selected}
            ctx={ctx}
            onField={onField}
            formErrors={validation.form}
            promote={
              promote
                ? {
                    baseLabel: promote.baseLabel,
                    task: launchTask,
                    setTask: setLaunchTask,
                    canLaunch: validation.ok && !!launchTask.trim(),
                    launching: promote.launching,
                    launchError: promote.launchError,
                    onLaunch: () =>
                      validation.ok &&
                      launchTask.trim() &&
                      promote.onLaunch(toSpec(state), launchTask.trim()),
                  }
                : undefined
            }
          />
        </div>

        <div className="wb-foot">
          <div className="wb-summary">
            {validation.ok ? (
              <>
                Ready to save · <b>{state.blocks.length}</b> block
                {state.blocks.length === 1 ? "" : "s"}
              </>
            ) : (
              <span className="wb-summary-err">
                <Icon name="close" size={11} />{" "}
                {validation.form[0] ??
                  `${Object.keys(validation.byNode).length} block(s) need attention`}
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
            setState((s) => setAgent(s, pop.nid, pop.role, id, agents));
            closePop();
          }}
        />
      )}
    </div>
  );
}
