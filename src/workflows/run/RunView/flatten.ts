// RunView/flatten.ts — a Spec's steps (spec §5.1) as the ordered list the
// stepper, the thread, the composer's flow strip and the sidebar row render.
// Attempts key on `step_id`, so each entry is the step's id plus what those
// surfaces show about it.

import type { Spec, Step } from "../../spec";

export interface StepDesc {
  id: string;
  /** Alias into `Spec.agents`. */
  agentAlias: string;
  goal: string;
  /** The gate kind that marks this step done (spec §9), for the hover card. */
  gate?: string;
}

/** A block as a run's frozen launch snapshot may still hold it. The retired
 *  clone-per-step engine nested steps inside parallel, loop and orchestrate
 *  containers, and those steps have attempts in the journal that still render.
 *  Read here only, so such a run keeps its steps' identities and order; the
 *  builder and launch schema (`Spec`) stays a flat list of steps. */
interface LegacyBlock {
  step?: Step;
  parallel?: { steps?: Step[] };
  loop?: { body?: LegacyBlock[] };
  orchestrate?: { body?: Step[] };
}

function walk(blocks: LegacyBlock[], out: StepDesc[]): void {
  for (const block of blocks) {
    if (block.step) pushStep(block.step, out);
    else if (block.parallel) for (const s of block.parallel.steps ?? []) pushStep(s, out);
    else if (block.loop) walk(block.loop.body ?? [], out);
    else if (block.orchestrate) for (const s of block.orchestrate.body ?? []) pushStep(s, out);
  }
}

function pushStep(step: Step, out: StepDesc[]): void {
  out.push({ id: step.id, agentAlias: step.agent, goal: step.goal, gate: step.gate?.type });
}

/** The spec's steps in document order — the rail's backbone. */
export function flattenSteps(spec: Spec | null): StepDesc[] {
  const out: StepDesc[] = [];
  walk(spec?.workflow ?? [], out);
  return out;
}
