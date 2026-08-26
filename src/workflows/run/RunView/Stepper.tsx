// RunView/Stepper.tsx — the run's spine as a horizontal progress strip under
// the header. Each step is one node: a status glyph, the step's name, and its
// duration — enough to read the run's shape at a glance. Everything else
// (goal, agent, gate, attempt history) is progressive disclosure: a hover card
// on the node, and a click that focuses that step's chat in the main pane.
//
// Attempts stay first-class but out of the way: a node whose step retried or
// looped carries a ×N count, and the AttemptStrip (rendered by RunView only
// when the focused step has more than one attempt) lists every attempt —
// abandoned ones dimmed, never hidden.

import { Fragment, type MouseEvent as ReactMouseEvent, type ReactNode, useState } from "react";
import type { WfStepExec } from "../../../api";
import { Icon } from "../../../components/Icon";
import { fmtDur, LiveTimer } from "../../../components/Workspace/RunTimer";
import { AgentAvatar } from "../../builder/AgentAvatar";
import type { ResolvedAgent } from "../../shared";
import { attemptChip } from "../status";
import type { StepDesc } from "./flatten";

/** Attempts of one step, in execution order (loop iteration, then retry). */
export function stepAttempts(attempts: WfStepExec[], stepId: string): WfStepExec[] {
  return attempts
    .filter((a) => a.step_id === stepId)
    .sort((a, b) => a.iteration - b.iteration || a.attempt - b.attempt);
}

/** The attempt a step click should focus — the newest by iteration/retry. */
export function latestAttempt(rows: WfStepExec[]): WfStepExec | undefined {
  return rows[rows.length - 1];
}

/** Node visual state, collapsed from the latest attempt's status. `attention`
 *  covers the amber "a human or a gate is holding this" family. */
type NodeState = "done" | "live" | "attention" | "error" | "pending";

function nodeState(s: WfStepExec["status"] | undefined): NodeState {
  switch (s) {
    case "done":
      return "done";
    case "spawning":
    case "running":
    case "gating":
      return "live";
    case "blocked":
    case "awaiting_approval":
      return "attention";
    case "error":
      return "error";
    default:
      // `abandoned` only ever coexists with a newer attempt, so the *latest*
      // attempt being abandoned means the step is effectively waiting again.
      return "pending";
  }
}

const CARD_W = 280;

interface HoverCard {
  stepId: string;
  x: number;
  y: number;
}

export function Stepper({
  steps,
  attempts,
  resolve,
  selectedStepId,
  onSelectStep,
  trailing,
}: {
  steps: StepDesc[];
  attempts: WfStepExec[];
  resolve: (alias: string) => ResolvedAgent | null;
  selectedStepId: string | null;
  onSelectStep: (step: StepDesc) => void;
  /** Right-pinned extras (the compact budget meters). */
  trailing?: ReactNode;
}) {
  const [card, setCard] = useState<HoverCard | null>(null);

  const showCard = (stepId: string, e: ReactMouseEvent<HTMLElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const x = Math.min(
      Math.max(8, rect.left + rect.width / 2 - CARD_W / 2),
      window.innerWidth - CARD_W - 8,
    );
    setCard({ stepId, x, y: rect.bottom + 8 });
  };

  const hoveredStep = card ? steps.find((s) => s.id === card.stepId) : undefined;

  return (
    <div className="wf-stepper">
      <div className="wf-stepper-track" role="list" aria-label="Workflow steps">
        {steps.map((step, i) => {
          const rows = stepAttempts(attempts, step.id);
          const latest = latestAttempt(rows);
          const chip = attemptChip(latest?.status ?? "pending");
          const state = nodeState(latest?.status);
          return (
            <Fragment key={step.id}>
              {i > 0 && <span className={`wf-conn ${state !== "pending" ? "past" : ""}`} />}
              <button
                type="button"
                role="listitem"
                className={`wf-node ${state} ${selectedStepId === step.id ? "sel" : ""}`}
                onClick={() => onSelectStep(step)}
                onMouseEnter={(e) => showCard(step.id, e)}
                onMouseLeave={() => setCard(null)}
                aria-label={`Step ${i + 1} of ${steps.length}: ${step.id} — ${chip.label}`}
                aria-current={selectedStepId === step.id ? "step" : undefined}
              >
                <NodeGlyph state={state} />
                <span className="wf-node-label">{step.id}</span>
                <NodeDuration state={state} latest={latest} />
                {rows.length > 1 && <span className="wf-node-x">×{rows.length}</span>}
              </button>
            </Fragment>
          );
        })}
      </div>
      {trailing}
      {card && hoveredStep && (
        <StepCard
          step={hoveredStep}
          index={steps.indexOf(hoveredStep)}
          rows={stepAttempts(attempts, hoveredStep.id)}
          agent={resolve(hoveredStep.agentAlias)}
          x={card.x}
          y={card.y}
        />
      )}
    </div>
  );
}

/** The node's 16px status mark: check (done), pulsing dot (live), pause
 *  (attention), cross (error), hollow ring (pending). */
function NodeGlyph({ state }: { state: NodeState }) {
  return (
    <span className={`wf-node-glyph ${state}`} aria-hidden="true">
      {state === "done" && <Icon name="check" size={10} />}
      {state === "live" && <span className="wf-node-pulse" />}
      {state === "attention" && <Icon name="pause" size={9} />}
      {state === "error" && <Icon name="close" size={10} />}
    </span>
  );
}

/** Completed steps show how long they took; the live step ticks in place.
 *  Steps that haven't started stay clean — no placeholder dashes. */
function NodeDuration({ state, latest }: { state: NodeState; latest?: WfStepExec }) {
  if (!latest?.started_at) return null;
  if (state === "live") {
    return (
      <span className="wf-node-dur">
        <LiveTimer startedAt={latest.started_at} />
      </span>
    );
  }
  if (!latest.ended_at) return null;
  return (
    <span className="wf-node-dur">{fmtDur((latest.ended_at - latest.started_at) / 1000)}</span>
  );
}

/** The hover card: the step's goal and its facts (agent, gate, attempts,
 *  duration). Pointer-events: none — it's a tooltip, not a surface. Fixed
 *  positioning, because the stepper track clips vertical overflow. */
function StepCard({
  step,
  index,
  rows,
  agent,
  x,
  y,
}: {
  step: StepDesc;
  index: number;
  rows: WfStepExec[];
  agent: ResolvedAgent | null;
  x: number;
  y: number;
}) {
  const latest = latestAttempt(rows);
  const chip = attemptChip(latest?.status ?? "pending");
  const iterations = rows.length > 0 ? rows[rows.length - 1].iteration + 1 : 0;
  return (
    <div className="wf-node-card" style={{ left: x, top: y, width: CARD_W }}>
      <div className="wf-nc-head">
        <span className="wf-nc-idx">
          Step {index + 1}
          {step.container && ` · ${step.container}`}
        </span>
        <span className="wf-nc-status" style={{ color: chip.tone }}>
          <Icon name={chip.icon} size={10} />
          {chip.label}
        </span>
      </div>
      <div className="wf-nc-goal">{step.goal || "No goal set."}</div>
      <div className="wf-nc-rows">
        {agent && (
          <div className="wf-nc-row">
            <span className="k">Agent</span>
            <span className="v">
              <AgentAvatar
                custom={agent.custom}
                slug={agent.providerId}
                short={agent.short}
                hue={agent.hue}
                size={14}
              />
              {agent.name}
            </span>
          </div>
        )}
        {step.gate && (
          <div className="wf-nc-row">
            <span className="k">Done when</span>
            <span className="v">{gateLabel(step.gate)}</span>
          </div>
        )}
        {rows.length > 0 && (
          <div className="wf-nc-row">
            <span className="k">Attempts</span>
            <span className="v">
              {rows.length}
              {iterations > 1 && ` across ${iterations} iterations`}
            </span>
          </div>
        )}
      </div>
      {/* Neutral across both run modes: a click opens the step's chat in the
          per-step view, and scrolls to its segment in the thread view. */}
      <div className="wf-nc-foot">Click to view this step</div>
    </div>
  );
}

/** Plain-language gate copy (spec §9) for the hover card. */
function gateLabel(gate: string): string {
  switch (gate) {
    case "verdict":
      return "agent reports done";
    case "commit":
      return "work is committed";
    case "artifact":
      return "artifact is produced";
    case "tests":
      return "tests pass";
    case "approval":
      return "you approve";
    default:
      return gate;
  }
}

/** The focused step's attempt history — rendered by RunView only when there is
 *  history to show (more than one attempt), keeping the common case clean. */
export function AttemptStrip({
  stepId,
  rows,
  selectedId,
  onSelect,
}: {
  stepId: string;
  rows: WfStepExec[];
  selectedId: string | null;
  onSelect: (attempt: WfStepExec) => void;
}) {
  const iterated = rows.some((r) => r.iteration > 0);
  return (
    <div className="wf-attempt-strip">
      <span className="wf-as-label">{stepId}</span>
      {rows.map((row) => {
        const chip = attemptChip(row.status);
        return (
          <button
            type="button"
            key={row.id}
            className={`wf-attempt ${selectedId === row.id ? "sel" : ""} ${
              row.status === "abandoned" ? "dim" : ""
            }`}
            onClick={() => onSelect(row)}
          >
            <span className="wf-attempt-chip" style={{ color: chip.tone }}>
              <Icon name={chip.icon} size={10} />
            </span>
            <span className="wf-attempt-label">
              {iterated ? `iter ${row.iteration + 1} · ` : ""}attempt {row.attempt}
            </span>
            <span className="wf-attempt-state" style={{ color: chip.tone }}>
              {chip.label}
            </span>
          </button>
        );
      })}
    </div>
  );
}
