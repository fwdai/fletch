// RunView/flatten.ts — collapse a launch-snapshot Spec's block tree (spec §5.1)
// into the ordered list of steps the attempt rail renders. Attempts key on
// `step_id`, so only Step nodes matter; container kind is carried for a subtle
// visual grouping. Parallel/loop/orchestrate execution lands in later slices —
// this walker already renders their steps so the rail is correct as they arrive.

import type { Block, Spec, Step } from "../../spec";

export type ContainerKind = "parallel" | "loop" | "orchestrate";

export interface StepDesc {
  id: string;
  /** Alias into `Spec.agents`. */
  agentAlias: string;
  goal: string;
  /** The enclosing container, if the step isn't a top-level sequence step. */
  container?: ContainerKind;
}

function walk(blocks: Block[], out: StepDesc[], container?: ContainerKind): void {
  for (const block of blocks) {
    if ("step" in block) {
      pushStep(block.step, out, container);
    } else if ("parallel" in block) {
      for (const s of block.parallel.steps) pushStep(s, out, "parallel");
    } else if ("loop" in block) {
      walk(block.loop.body, out, "loop");
    } else if ("orchestrate" in block) {
      for (const s of block.orchestrate.body ?? []) pushStep(s, out, "orchestrate");
    }
  }
}

function pushStep(step: Step, out: StepDesc[], container?: ContainerKind): void {
  out.push({ id: step.id, agentAlias: step.agent, goal: step.goal, container });
}

/** The spec's steps in document order — the rail's backbone. */
export function flattenSteps(spec: Spec | null): StepDesc[] {
  if (!spec?.workflow) return [];
  const out: StepDesc[] = [];
  walk(spec.workflow, out);
  return out;
}
