// RunView/flatten.ts — a Spec's steps (spec §5.1) as the ordered list the
// stepper, the thread, the composer's flow strip and the sidebar row render.
// Attempts key on `step_id`, so each entry is the step's id plus what those
// surfaces show about it.

import type { Spec } from "../../spec";

export interface StepDesc {
  id: string;
  /** Alias into `Spec.agents`. */
  agentAlias: string;
  goal: string;
  /** The gate kind that marks this step done (spec §9), for the hover card. */
  gate?: string;
}

/** The spec's steps in document order — the rail's backbone. */
export function flattenSteps(spec: Spec | null): StepDesc[] {
  const out: StepDesc[] = [];
  for (const { step } of spec?.workflow ?? []) {
    // A run's spec is its frozen launch snapshot, so one the retired engine
    // launched can still hold an entry with no `step` (a parallel, loop or
    // orchestrate block). Skip it rather than take the monitor down with it.
    if (!step) continue;
    out.push({ id: step.id, agentAlias: step.agent, goal: step.goal, gate: step.gate?.type });
  }
  return out;
}
