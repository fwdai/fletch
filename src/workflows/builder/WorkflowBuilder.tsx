// WorkflowBuilder.tsx — the block-tree editor (spec §14.1). The canvas renders
// the recursive block sequence; a shared `ctx` lets any nested card dispatch an
// edit or open a popover. Validation from `model.ts` renders inline and gates
// the save. Persistence is the caller's job (Save hands back the editor state).

import { useMemo, useState } from "react";
import { Icon } from "../../components/Icon";
import type { ModelMeta } from "../../data/modelCatalog/types";
import type { CustomAgent } from "../../storage/customAgents";
import { resolveAlias } from "../shared";
import type { Budgets, Gate, Spec } from "../spec";
import { BlockSequence } from "./blocks/BlockSequence";
import type { BuilderCtx, Pop, PopRect } from "./ctx";
import {
  type AgentRole,
  addBlockOfType,
  addStepToContainer,
  findStep,
  patchBlock,
  patchStep,
  removeNode,
  setAgent,
  setField,
} from "./edits";
import { useDismissOnViewportChange } from "./hooks";
import type { EditorState, NodeId } from "./model";
import { toSpec, validateEditor } from "./model";
import { AgentPick, BudgetsPopover, GatePick } from "./pickers";

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
  const [launchTask, setLaunchTask] = useState(promote?.taskSeed ?? "");
  const [pop, setPop] = useState<Pop | null>(null);
  const closePop = () => setPop(null);
  useDismissOnViewportChange(!!pop, closePop);

  const validation = useMemo(() => validateEditor(state), [state]);

  const ctx: BuilderCtx = useMemo(
    () => ({
      // `id` is an alias into `state.agents`; `resolveAlias` maps it to the
      // underlying custom-agent id or base provider before rendering.
      resolve: (id) => resolveAlias(state.agents, id ?? undefined, agents, modelsByAgent),
      errorsFor: (nid) => validation.byNode[nid],
      patchStep: (nid, patch) => setState((s) => patchStep(s, nid, patch)),
      patchBlock: (nid, patch) => setState((s) => patchBlock(s, nid, patch)),
      removeNode: (nid) => setState((s) => removeNode(s, nid)),
      addStepToContainer: (nid) => setState((s) => addStepToContainer(s, nid)),
      addBlock: (seqNid, type) => setState((s) => addBlockOfType(s, seqNid, type)),
      openAgent: (nid, role: AgentRole, e) =>
        setPop({ type: "agent", nid, role, rect: rectFrom(e) }),
      openGate: (nid, e) => setPop({ type: "gate", nid, rect: rectFrom(e) }),
      openBudgets: (target, e) => setPop({ type: "budgets", target, rect: rectFrom(e) }),
    }),
    [agents, modelsByAgent, validation, state.agents],
  );

  const runBudgetLabel = state.budgets?.turns ? `${state.budgets.turns} turns` : "budgets";
  const finalize = state.finalize;

  const setGate = (nid: NodeId, gate: Gate) => setState((s) => patchStep(s, nid, { gate }));
  const budgetsOf = (target: NodeId | "run"): Budgets | undefined =>
    target === "run" ? state.budgets : (findStep(state, target)?.budgets ?? undefined);
  const setBudgets = (target: NodeId | "run", next: Budgets | undefined) =>
    target === "run"
      ? setState((s) => setField(s, { budgets: next }))
      : setState((s) => patchStep(s, target, { budgets: next }));

  return (
    <div className="set-pane">
      <div className="wb">
        <button className="ca-ed-back" onClick={onCancel}>
          <Icon name="chevL" /> All workflows
        </button>

        <div className="wb-top">
          <div className="wb-titlewrap">
            <input
              className="wb-name"
              placeholder="Name this workflow…"
              value={state.name}
              autoFocus
              onChange={(e) => setState((s) => setField(s, { name: e.target.value }))}
            />
            <textarea
              className="wb-desc"
              rows={1}
              placeholder="What is this pipeline for?"
              value={state.description}
              onChange={(e) => setState((s) => setField(s, { description: e.target.value }))}
            />
          </div>
          <button
            className="wb-chip-btn lg tip"
            data-tip-down
            data-tip="Run-level budgets"
            onClick={(e) => ctx.openBudgets("run", e)}
          >
            <Icon name="clock" size={12} /> {runBudgetLabel}
          </button>
        </div>

        <div className="wb-canvas">
          <BlockSequence blocks={state.blocks} seqNid={null} ctx={ctx} />
        </div>

        <div className="wb-finish">
          <span className="wb-finish-l">When the run finishes</span>
          <label className="wb-toggle">
            <input
              type="checkbox"
              checked={!!finalize?.push}
              onChange={(e) =>
                setState((s) =>
                  setField(s, {
                    finalize: {
                      push: e.target.checked,
                      open_pr: finalize?.open_pr ?? false,
                      pr_base: finalize?.pr_base,
                    },
                  }),
                )
              }
            />
            Push the branch
          </label>
          <label className="wb-toggle">
            <input
              type="checkbox"
              checked={!!finalize?.open_pr}
              onChange={(e) =>
                setState((s) =>
                  setField(s, {
                    finalize: {
                      push: finalize?.push ?? false,
                      open_pr: e.target.checked,
                      pr_base: finalize?.pr_base,
                    },
                  }),
                )
              }
            />
            Open a PR
          </label>
          {finalize?.open_pr && (
            <input
              className="ca-input sm"
              style={{ width: 120 }}
              placeholder="base: main"
              value={finalize.pr_base ?? ""}
              onChange={(e) =>
                setState((s) =>
                  setField(s, {
                    finalize: {
                      push: finalize.push,
                      open_pr: finalize.open_pr,
                      pr_base: e.target.value.trim() || undefined,
                    },
                  }),
                )
              }
            />
          )}
        </div>

        {promote && (
          <div className="wb-promote">
            <div className="wb-promote-head">
              <Icon name="combine" size={13} style={{ color: "var(--accent)" }} />
              <span>Launch this workflow now</span>
              <span
                className="wb-promote-base tip"
                data-tip="Forks from the promoted session's commit"
              >
                base <span className="mono">{promote.baseLabel}</span>
              </span>
            </div>
            <textarea
              className="wb-desc"
              rows={2}
              placeholder="Task for the run…"
              value={launchTask}
              onChange={(e) => setLaunchTask(e.target.value)}
            />
            {promote.launchError && <div className="wb-summary-err">{promote.launchError}</div>}
            <div className="wb-promote-foot">
              <span className="wb-promote-note">Or just save it as a reusable workflow below.</span>
              <span className="grow" />
              <button
                className="btn-t primary"
                disabled={!validation.ok || !launchTask.trim() || promote.launching}
                style={
                  !validation.ok || !launchTask.trim() || promote.launching
                    ? { opacity: 0.5 }
                    : undefined
                }
                onClick={() =>
                  validation.ok &&
                  launchTask.trim() &&
                  promote.onLaunch(toSpec(state), launchTask.trim())
                }
              >
                <Icon name={promote.launching ? "refresh" : "arrowUp"} size={13} />{" "}
                {promote.launching ? "Launching…" : "Launch run"}
              </button>
            </div>
          </div>
        )}

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
      {pop?.type === "gate" && (
        <GatePick
          rect={pop.rect}
          gate={findStep(state, pop.nid)?.gate.type ?? "verdict"}
          onPick={(kind) => {
            const cur = findStep(state, pop.nid)?.gate;
            const gate: Gate =
              kind === "artifact"
                ? { type: "artifact", path: cur?.type === "artifact" ? cur.path : "" }
                : { type: kind };
            setGate(pop.nid, gate);
            closePop();
          }}
        />
      )}
      {pop?.type === "budgets" && (
        <BudgetsPopover
          rect={pop.rect}
          scope={pop.target === "run" ? "run" : "step"}
          value={budgetsOf(pop.target)}
          onChange={(next) => setBudgets(pop.target, next)}
        />
      )}
    </div>
  );
}
