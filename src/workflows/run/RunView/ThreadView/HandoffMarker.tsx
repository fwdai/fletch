// ThreadView/HandoffMarker — the seam where one agent hands the work to the
// next. The thread reads as one conversation, so the only thing that must never
// be implicit is *who* is speaking from here on: name, identity chip, and the
// step's place in the sequence.

import { Icon } from "../../../../components/Icon";
import { fmtDur } from "../../../../components/Workspace/RunTimer";
import { AgentAvatar } from "../../../builder/AgentAvatar";
import type { ResolvedAgent } from "../../../shared";
import { attemptChip } from "../../status";
import type { Segment } from "./segments";

export function HandoffMarker({
  segment,
  stepCount,
  agent,
}: {
  segment: Segment;
  stepCount: number;
  /** The step's resolved identity, or null when its alias no longer resolves. */
  agent: ResolvedAgent | null;
}) {
  const { exec, step, stepIndex, retryIndex } = segment;
  const chip = attemptChip(exec.status);
  const ran =
    exec.started_at != null && exec.ended_at != null
      ? fmtDur((exec.ended_at - exec.started_at) / 1000)
      : null;

  return (
    <div className="wf-handoff" data-step-id={exec.step_id}>
      <span className="wf-handoff-line" aria-hidden="true" />
      <span className="wf-handoff-body">
        {agent ? (
          <AgentAvatar
            custom={agent.custom}
            slug={agent.providerId}
            short={agent.short}
            hue={agent.hue}
            size={16}
          />
        ) : (
          <Icon name="bot" size={13} />
        )}
        <span className="wf-handoff-name">{agent?.name ?? "Unknown agent"}</span>
        <span className="wf-handoff-sep">·</span>
        <span className="wf-handoff-step">{step?.id ?? exec.step_id}</span>
        {stepIndex >= 0 && (
          <span className="wf-handoff-idx">
            step {stepIndex + 1} of {stepCount}
          </span>
        )}
        {retryIndex > 0 && <span className="wf-handoff-retry">retry {retryIndex}</span>}
        {ran && <span className="wf-handoff-dur">{ran}</span>}
        {exec.status !== "done" && (
          <span className="wf-handoff-state" style={{ color: chip.tone }}>
            {chip.label}
          </span>
        )}
      </span>
      <span className="wf-handoff-line" aria-hidden="true" />
    </div>
  );
}
