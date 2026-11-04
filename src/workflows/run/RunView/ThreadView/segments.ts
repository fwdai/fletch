// ThreadView/segments.ts — the thread's spine: one transcript segment per step
// attempt, in execution order.
//
// Execution order, never timestamps. Steps run in spec order, so the step's
// top-level block is the primary key, then the attempt's iteration, then the
// step's position inside its block, then the retry. The iteration sits above
// the position because the retired engine ran a loop's whole body once per
// iteration; for the kernel's flat specs iteration and position are always 0,
// so the key is simply the step's place. Interleaving by `started_at` would
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
  const key = (exec: WfStepExec): [number, number, number, number] => {
    const step = steps[order.get(exec.step_id) ?? -1];
    return step
      ? [step.block, exec.iteration, step.position, exec.attempt]
      : [Number.POSITIVE_INFINITY, exec.iteration, 0, exec.attempt];
  };
  const before = (a: WfStepExec, b: WfStepExec) => {
    const ka = key(a);
    const kb = key(b);
    return ka[0] - kb[0] || ka[1] - kb[1] || ka[2] - kb[2] || ka[3] - kb[3];
  };

  const rows = attempts
    .slice()
    // The exec row is created before the agent is spawned, so a step about to
    // start already has one. It has nothing to render, and the phase row already
    // names that moment — a marker here would announce the step twice. A row that
    // ended without an agent (spawn failed, abandoned) is real history and stays.
    .filter((a) => a.agent_id != null || !UNSTARTED_STATUSES.has(a.status))
    .sort(before);

  // Retries count within one (step, iteration): a loop's next iteration is a
  // fresh first attempt at the step, not a retry of the last one.
  const seen = new Map<string, number>();
  return rows.map((exec) => {
    const pass = `${exec.step_id}\u0000${exec.iteration}`;
    const retryIndex = seen.get(pass) ?? 0;
    seen.set(pass, retryIndex + 1);
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
