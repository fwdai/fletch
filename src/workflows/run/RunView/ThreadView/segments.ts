// ThreadView/segments.ts — the thread's spine: one transcript segment per step
// attempt, in execution order.
//
// Execution order, never timestamps. A sequential run's steps run in spec order,
// so the step's position in the flattened spec is the primary key and the
// attempt's (iteration, retry) its tie-break. Interleaving by `started_at` would
// reorder the thread whenever two rows share a millisecond, and a step's rows
// must stay adjacent regardless.

import type { AgentRecord, WfStepExec } from "../../../../api";
import type { StepDesc } from "../flatten";

export interface Segment {
  /** The attempt this segment renders. */
  exec: WfStepExec;
  /** The spec step it belongs to; undefined for a row whose step left the spec. */
  step: StepDesc | undefined;
  /** 0-based position in the flattened step list. */
  stepIndex: number;
  /** Which retry of the step this is, 0-based — a marker shows it when > 0. */
  retryIndex: number;
  /** The step agent's record, when the run still owns it (live or archived). */
  agent: AgentRecord | undefined;
  /** The run is still working this attempt: its chat keeps the live behaviors. */
  live: boolean;
}

/** Attempt statuses that mean the kernel has not moved past this step yet. */
const LIVE_STATUSES: ReadonlySet<WfStepExec["status"]> = new Set(["spawning", "running", "gating"]);

/** Statuses an attempt holds before it has an agent to show. */
const UNSTARTED_STATUSES: ReadonlySet<WfStepExec["status"]> = new Set(["pending", "spawning"]);

export function deriveSegments(
  steps: StepDesc[],
  attempts: WfStepExec[],
  agents: AgentRecord[],
): Segment[] {
  const order = new Map(steps.map((s, i) => [s.id, i]));
  // A row whose step_id is absent from the spec (hand-edited spec, renamed step)
  // sorts after every known step rather than to the front.
  const rank = (exec: WfStepExec) => order.get(exec.step_id) ?? steps.length;

  const rows = attempts
    .slice()
    // The exec row is created before the agent is spawned, so a step about to
    // start already has one. It has nothing to render, and the phase row already
    // names that moment — a marker here would announce the step twice. A row that
    // ended without an agent (spawn failed, abandoned) is real history and stays.
    .filter((a) => a.agent_id != null || !UNSTARTED_STATUSES.has(a.status))
    .sort((a, b) => rank(a) - rank(b) || a.iteration - b.iteration || a.attempt - b.attempt);

  const seenPerStep = new Map<string, number>();
  return rows.map((exec) => {
    const retryIndex = seenPerStep.get(exec.step_id) ?? 0;
    seenPerStep.set(exec.step_id, retryIndex + 1);
    const stepIndex = order.get(exec.step_id) ?? -1;
    return {
      exec,
      step: stepIndex >= 0 ? steps[stepIndex] : undefined,
      stepIndex,
      retryIndex,
      agent: exec.agent_id ? agents.find((a) => a.id === exec.agent_id) : undefined,
      live: LIVE_STATUSES.has(exec.status),
    };
  });
}

/** The agent the thread's one composer talks to: the live attempt's, if any.
 *  Later segments win, so a fresh step takes over the composer the moment it
 *  spawns. */
export function liveAgent(segments: Segment[]): AgentRecord | undefined {
  for (let i = segments.length - 1; i >= 0; i -= 1) {
    if (segments[i].live && segments[i].agent) return segments[i].agent;
  }
  return undefined;
}
