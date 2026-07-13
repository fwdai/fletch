// WorkflowBuilder.tsx — the block-tree editor (spec §14.1). The canvas renders
// the recursive block sequence; a shared `ctx` lets any nested card dispatch an
// edit or open a popover. Validation from `model.ts` renders inline and gates
// the save. Persistence is the caller's job (Save hands back the editor state).

import { useMemo, useState } from "react";
import { Icon } from "../../components/Icon";
import type { ModelMeta } from "../../data/modelCatalog/types";
import type { CustomAgent } from "../../storage/customAgents";
import { resolveAgent } from "../shared";
import type { Budgets, Gate } from "../spec";
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
import type { EditorState, NodeId } from "./model";
import { validateEditor } from "./model";
import { AgentPick, BudgetsPopover, GatePick } from "./pickers";

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
  const [pop, setPop] = useState<Pop | null>(null);
  const closePop = () => setPop(null);

  const validation = useMemo(() => validateEditor(state), [state]);

  const ctx: BuilderCtx = useMemo(
    () => ({
      resolve: (id) => resolveAgent(id, agents, modelsByAgent),
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
    [agents, modelsByAgent, validation],
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
