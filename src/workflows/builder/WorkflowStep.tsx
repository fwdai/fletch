// WorkflowStep.tsx — one step card in the builder canvas. The loop edge itself
// is drawn as an arc overlay by WorkflowBuilder; here the footer only carries
// the advance selector and the loop toggle.

import { Icon } from "../../components/Icon";
import { ADVANCE_MODES } from "../data";
import type { AgentResolver } from "../shared";
import type { WorkflowStep as Step } from "../storage";
import { AgentAvatar } from "./AgentAvatar";

export function WorkflowStep({
  step,
  index,
  resolve,
  isLoopTarget,
  canLoop,
  innerRef,
  onPick,
  onAdvance,
  onLoop,
  onGoal,
  onRemove,
}: {
  step: Step;
  index: number;
  resolve: AgentResolver;
  isLoopTarget: boolean;
  canLoop: boolean;
  innerRef: (el: HTMLDivElement | null) => void;
  onPick: (e: React.MouseEvent) => void;
  onAdvance: (e: React.MouseEvent) => void;
  onLoop: (e: React.MouseEvent) => void;
  onGoal: (v: string) => void;
  onRemove: (() => void) | null;
}) {
  const a = resolve(step.agent);
  const mode = ADVANCE_MODES.find((m) => m.id === step.advance) || ADVANCE_MODES[0];

  return (
    <div className={`wb-step ${isLoopTarget ? "is-loop-target" : ""}`} ref={innerRef}>
      <div className="wb-step-h">
        <span className="wb-step-idx">{String(index + 1).padStart(2, "0")}</span>
        {/* One real-box trigger (mono + label) so the agent dropdown anchors to
            it. The prototype split this into a `display:contents` mono button,
            which has no layout box — its getBoundingClientRect() is all zeros,
            so the popover jumped to the top-left corner. */}
        <button className="wb-step-agent" onClick={onPick}>
          {a ? (
            <AgentAvatar
              custom={a.custom}
              slug={a.providerId}
              short={a.short}
              hue={a.hue}
              size={26}
            />
          ) : (
            <span className="wb-step-mono empty">
              <Icon name="plus" size={12} />
            </span>
          )}
          <span className="wb-step-agent-text">
            <div className={`wb-an ${a ? "" : "empty"}`}>{a ? a.name : "Choose agent"}</div>
            {a && <div className="wb-am">{a.custom ? `${a.baseLabel} · ${a.model}` : a.model}</div>}
          </span>
        </button>
        {onRemove && (
          <button
            className="tip wb-step-menu"
            data-tip-down
            data-tip="Remove step"
            onClick={onRemove}
          >
            <Icon name="close" />
          </button>
        )}
      </div>

      <textarea
        className="wb-step-goal"
        placeholder="What should this step accomplish?"
        value={step.goal}
        onChange={(e) => onGoal(e.target.value)}
      />

      <div className="wb-step-foot">
        <span className="wb-advance">
          <Icon name={mode.icon} />
          <span>
            Advance{" "}
            <button className="wb-adv-sel" onClick={onAdvance}>
              {mode.short}
            </button>
          </span>
        </span>
        {canLoop && (
          <button
            className={`wb-loop-btn tip ${step.loop ? "on" : ""}`}
            data-tip-down
            data-tip={step.loop ? "Edit loop" : "Loop back to an earlier step"}
            onClick={onLoop}
          >
            <Icon name="loop" />
          </button>
        )}
      </div>
    </div>
  );
}
