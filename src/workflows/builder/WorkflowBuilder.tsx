// WorkflowBuilder.tsx — the visual builder: a left-to-right chain of steps with
// one optional loop-back edge drawn as a measured arc above the track.

import { Fragment, useLayoutEffect, useRef, useState } from "react";
import { Icon } from "../../components/Icon";
import type { ModelMeta } from "../../data/modelCatalog/types";
import type { CustomAgent } from "../../storage/customAgents";
import { newStep, resolveAgent } from "../shared";
import type { WorkflowStep as Step, WorkflowDraft } from "../storage";
import { AdvancePick, AgentPick, LoopEditor, type PopRect } from "./pickers";
import { WorkflowStep } from "./WorkflowStep";

type Pop = { type: "agent" | "advance" | "loop"; stepId: string; rect: PopRect };

export function WorkflowBuilder({
  draft,
  isNew,
  agents,
  modelsByAgent,
  onCancel,
  onSave,
}: {
  draft: WorkflowDraft;
  isNew: boolean;
  agents: CustomAgent[];
  modelsByAgent: Record<string, ModelMeta[]>;
  onCancel: () => void;
  onSave: (w: WorkflowDraft) => void;
}) {
  const [w, setW] = useState<WorkflowDraft>(draft);
  const [pop, setPop] = useState<Pop | null>(null);

  const resolve = (id: string | null) => resolveAgent(id, agents, modelsByAgent);

  const set = (patch: Partial<WorkflowDraft>) => setW((x) => ({ ...x, ...patch }));
  const setStep = (id: string, patch: Partial<Step>) =>
    setW((x) => ({ ...x, steps: x.steps.map((s) => (s.id === id ? { ...s, ...patch } : s)) }));
  const addStep = () => setW((x) => ({ ...x, steps: [...x.steps, newStep()] }));
  const removeStep = (id: string) =>
    setW((x) => ({
      ...x,
      steps: x.steps
        .filter((s) => s.id !== id)
        .map((s) => (s.loop?.to === id ? { ...s, loop: null } : s)),
    }));

  const openPop = (type: Pop["type"], stepId: string, e: React.MouseEvent) => {
    const r = e.currentTarget.getBoundingClientRect();
    setPop({
      type,
      stepId,
      rect: { top: r.bottom + 6, left: r.left, right: r.right, bottom: r.bottom },
    });
  };
  const closePop = () => setPop(null);

  const loopStep = w.steps.find((s) => s.loop);
  const canSave = !!w.name.trim() && w.steps.every((s) => s.agent);

  // Measure the loop arc from real DOM positions (steps are flexible width).
  const trackRef = useRef<HTMLDivElement | null>(null);
  const stepRefs = useRef<Record<string, HTMLDivElement>>({});
  const [arc, setArc] = useState<{ left: number; width: number } | null>(null);
  const loopKey = loopStep
    ? `${loopStep.id}:${loopStep.loop!.to}:${loopStep.loop!.when}:${loopStep.loop!.max}`
    : "";
  useLayoutEffect(() => {
    const measure = () => {
      if (!loopStep) {
        setArc(null);
        return;
      }
      const track = trackRef.current;
      const src = stepRefs.current[loopStep.id];
      const tgt = stepRefs.current[loopStep.loop!.to];
      if (!track || !src || !tgt) {
        setArc(null);
        return;
      }
      const tr = track.getBoundingClientRect();
      const sc = src.getBoundingClientRect();
      const tc = tgt.getBoundingClientRect();
      const srcC = sc.left - tr.left + sc.width / 2;
      const tgtC = tc.left - tr.left + tc.width / 2;
      setArc({ left: Math.min(srcC, tgtC), width: Math.abs(srcC - tgtC) });
    };
    measure();
    const ro = new ResizeObserver(measure);
    if (trackRef.current) ro.observe(trackRef.current);
    window.addEventListener("resize", measure);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", measure);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [w.steps.length, loopKey]);

  const popStep = pop ? w.steps.find((s) => s.id === pop.stepId) : undefined;

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
              value={w.name}
              autoFocus
              onChange={(e) => set({ name: e.target.value })}
            />
            <textarea
              className="wb-desc"
              rows={1}
              placeholder="What is this pipeline for?"
              value={w.description}
              onChange={(e) => set({ description: e.target.value })}
            />
          </div>
        </div>

        <div className="wb-canvas">
          <div className="wb-track-inner" ref={trackRef}>
            {loopStep && arc && (
              <div className="wb-loop-ribbon" style={{ left: arc.left, width: arc.width }}>
                <span className="wb-loop-arrow">
                  <Icon name="arrowDown" />
                </span>
                <button className="wb-loop-label" onClick={(e) => openPop("loop", loopStep.id, e)}>
                  <Icon name="loop" /> until {loopStep.loop!.when} · max {loopStep.loop!.max}
                  <span
                    className="wb-loop-x"
                    onClick={(e) => {
                      e.stopPropagation();
                      setStep(loopStep.id, { loop: null });
                    }}
                  >
                    <Icon name="close" size={9} />
                  </span>
                </button>
              </div>
            )}

            {w.steps.map((s, i) => (
              <Fragment key={s.id}>
                {i > 0 && (
                  <div className="wb-conn">
                    <Icon name="arrowR" />
                  </div>
                )}
                <WorkflowStep
                  step={s}
                  index={i}
                  resolve={resolve}
                  isLoopTarget={!!loopStep && loopStep.loop!.to === s.id}
                  canLoop={i > 0}
                  innerRef={(el) => {
                    if (el) stepRefs.current[s.id] = el;
                    else delete stepRefs.current[s.id];
                  }}
                  onPick={(e) => openPop("agent", s.id, e)}
                  onAdvance={(e) => openPop("advance", s.id, e)}
                  onLoop={(e) => openPop("loop", s.id, e)}
                  onGoal={(v) => setStep(s.id, { goal: v })}
                  onRemove={w.steps.length > 1 ? () => removeStep(s.id) : null}
                />
              </Fragment>
            ))}

            <div className="wb-conn">
              <Icon name="arrowR" />
            </div>
            <button className="wb-add tip" data-tip-down data-tip="Add step" onClick={addStep}>
              <span className="wb-add-ic">
                <Icon name="plus" />
              </span>
              <span className="wb-add-l">Add step</span>
            </button>
          </div>
        </div>

        <div className="wb-foot">
          <div className="wb-summary">
            <b>{w.steps.length}</b> step{w.steps.length === 1 ? "" : "s"}
            {loopStep && (
              <>
                {" · "}
                <b>1</b> loop
              </>
            )}
            {" · runs on a fresh worktree per launch"}
          </div>
          <span className="grow"></span>
          <button className="btn-t ghost" onClick={onCancel}>
            Cancel
          </button>
          <button
            className="btn-t primary"
            disabled={!canSave}
            style={!canSave ? { opacity: 0.5 } : undefined}
            onClick={() => canSave && onSave(w)}
          >
            <Icon name="check" size={13} /> {isNew ? "Create workflow" : "Save workflow"}
          </button>
        </div>
      </div>

      {pop && <div style={{ position: "fixed", inset: 0, zIndex: 55 }} onClick={closePop}></div>}
      {pop?.type === "agent" && (
        <AgentPick
          rect={pop.rect}
          agents={agents}
          onPick={(id) => {
            setStep(pop.stepId, { agent: id });
            closePop();
          }}
        />
      )}
      {pop?.type === "advance" && (
        <AdvancePick
          rect={pop.rect}
          value={popStep?.advance}
          onPick={(v) => {
            setStep(pop.stepId, { advance: v });
            closePop();
          }}
        />
      )}
      {pop?.type === "loop" && popStep && (
        <LoopEditor
          rect={pop.rect}
          step={popStep}
          steps={w.steps}
          resolve={resolve}
          onApply={(loop) => {
            setStep(pop.stepId, { loop });
            closePop();
          }}
          onClear={() => {
            setStep(pop.stepId, { loop: null });
            closePop();
          }}
        />
      )}
    </div>
  );
}
